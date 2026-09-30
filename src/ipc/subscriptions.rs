//! Replacement snapshots with one pending frame per subscriber.
use knave_desktop_api::{
    DesktopError, DesktopErrorCode, DesktopResponse, DesktopSnapshot, MAX_SNAPSHOT_FRAME_BYTES,
};
use std::{
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{Arc, Mutex, Weak},
};

type Frame = Arc<[u8]>;
pub(super) struct Subscriber {
    latest: Mutex<Option<Frame>>,
    wake: UnixStream,
}
impl Subscriber {
    pub(super) fn new() -> io::Result<(Arc<Self>, UnixStream)> {
        let (wake, reader) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        reader.set_nonblocking(true)?;
        Ok((
            Arc::new(Self {
                latest: Mutex::new(None),
                wake,
            }),
            reader,
        ))
    }
    fn publish(&self, frame: Frame) -> bool {
        let mut slot = self.latest.lock().unwrap();
        let notify = slot.is_none();
        *slot = Some(frame);
        if notify {
            match (&self.wake).write(&[1]) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return false,
            }
        }
        true
    }
}

#[derive(Default)]
pub(super) struct Subscriptions {
    pub(super) snapshot: Option<DesktopSnapshot>,
    subscribers: Vec<Weak<Subscriber>>,
}
impl Subscriptions {
    pub(super) fn publish(&mut self, mut snapshot: DesktopSnapshot) {
        if self.snapshot.as_ref().is_some_and(|old| {
            old.overview_visible == snapshot.overview_visible
                && old.windows == snapshot.windows
                && old.workspaces == snapshot.workspaces
        }) {
            return;
        }
        snapshot.generation = self
            .snapshot
            .as_ref()
            .map_or(1, |old| old.generation.wrapping_add(1));
        self.snapshot = Some(snapshot);
        self.subscribers
            .retain(|subscriber| subscriber.strong_count() > 0);
        if self.subscribers.is_empty() {
            return;
        }
        let frame = self.frame();
        self.subscribers.retain(|subscriber| {
            subscriber
                .upgrade()
                .is_some_and(|s| s.publish(frame.clone()))
        });
    }
    pub(super) fn subscribe(&mut self, subscriber: &Arc<Subscriber>) {
        self.subscribers
            .retain(|subscriber| subscriber.strong_count() > 0);
        self.subscribers.push(Arc::downgrade(subscriber));
    }
    pub(super) fn frame(&self) -> Frame {
        let snapshot = self
            .snapshot
            .as_ref()
            .expect("snapshot initialized before subscription");
        let mut frame = serde_json::to_vec(&DesktopResponse::Snapshot(snapshot.clone()))
            .expect("snapshot serializes");
        if frame.len() as u64 >= MAX_SNAPSHOT_FRAME_BYTES {
            frame = serde_json::to_vec(&DesktopResponse::Error(DesktopError {
                code: DesktopErrorCode::Unavailable,
                message: "Desktop snapshot exceeds subscription capacity".into(),
                retryable: false,
            }))
            .expect("error serializes");
        }
        frame.push(b'\n');
        frame.into()
    }
}

pub(super) fn serve(
    stream: &mut UnixStream,
    wake: &mut UnixStream,
    subscriber: &Subscriber,
) -> io::Result<()> {
    loop {
        let mut fds = [
            libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // Both descriptors remain owned for the whole blocking poll; shutdown wakes it.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        // Subscription connections accept no further requests. Readability includes EOF.
        if fds[0].revents != 0
            || fds[1].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
        {
            return Ok(());
        }
        let mut bytes = [0; 64];
        loop {
            match wake.read(&mut bytes) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        let frame = subscriber.latest.lock().unwrap().take();
        if let Some(frame) = frame {
            stream.write_all(&frame)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_only_changes_publish_and_wake_subscribers() {
        let mut subscriptions = Subscriptions::default();
        let mut snapshot = DesktopSnapshot {
            generation: 0,
            overview_visible: false,
            workspaces: Vec::new(),
            windows: Vec::new(),
        };
        subscriptions.publish(snapshot.clone());
        let (subscriber, mut wake) = Subscriber::new().unwrap();
        subscriptions.subscribe(&subscriber);

        for (visible, generation) in [(true, 2), (false, 3)] {
            snapshot.overview_visible = visible;
            subscriptions.publish(snapshot.clone());
            let mut byte = [0];
            assert_eq!(wake.read(&mut byte).unwrap(), 1);
            let frame = subscriber.latest.lock().unwrap().take().unwrap();
            let DesktopResponse::Snapshot(published) = serde_json::from_slice(&frame).unwrap()
            else {
                panic!("expected a snapshot");
            };
            assert_eq!(published.overview_visible, visible);
            assert_eq!(published.generation, generation);

            subscriptions.publish(snapshot.clone());
            assert!(subscriber.latest.lock().unwrap().is_none());
            assert_eq!(
                wake.read(&mut byte).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            assert_eq!(
                subscriptions.snapshot.as_ref().unwrap().generation,
                generation
            );
        }
    }
}
