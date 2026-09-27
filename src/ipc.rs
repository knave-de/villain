//! JSON-lines server for Knave’s versioned desktop contract.

mod subscriptions;
use subscriptions::{Subscriber, Subscriptions};

use std::{
    cell::RefCell,
    ffi::OsStr,
    fs,
    io::{BufRead, BufReader, Write},
    net::Shutdown,
    os::unix::{fs::FileTypeExt, net::UnixListener},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use base64::Engine;
use knave_desktop_api::{
    API_VERSION, DesktopError, DesktopErrorCode, DesktopQuery, DesktopRequest, DesktopResponse,
    DesktopSnapshot, ProtocolVersion, WorkspaceId, WorkspacePreview, socket_path_for_display,
};
use smithay::reexports::calloop::{
    EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken, channel, generic::Generic,
};

use crate::{
    dispatch::{Dispatch, DispatchError},
    state::Villain,
};

enum IpcMessage {
    Request(Envelope),
    Finished(u64),
}

struct Envelope {
    request: DesktopRequest,
    response: mpsc::Sender<Reply>,
    subscriber: Option<Arc<Subscriber>>,
}

enum Reply {
    Response(DesktopResponse),
    Subscribed(Arc<[u8]>),
}
struct ClientWorker {
    id: u64,
    socket: std::os::unix::net::UnixStream,
    thread: thread::JoinHandle<()>,
}
pub struct IpcServer {
    path: PathBuf,
    subscriptions: Subscriptions,
    clients: Rc<RefCell<Vec<ClientWorker>>>,
    handle: LoopHandle<'static, Villain>,
    listener: RegistrationToken,
    requests: RegistrationToken,
}
const MAX_CLIENTS: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
impl Drop for IpcServer {
    fn drop(&mut self) {
        self.handle.remove(self.listener);
        self.handle.remove(self.requests);
        let clients = self.clients.take();
        for client in &clients {
            let _ = client.socket.shutdown(Shutdown::Both);
        }
        for client in clients {
            let _ = client.thread.join();
        }
        remove_socket(&self.path);
    }
}

pub fn init(
    event_loop: &mut EventLoop<'static, Villain>,
    display: &OsStr,
) -> Result<IpcServer, Box<dyn std::error::Error>> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set")
    })?;
    let path = socket_path_for_display(runtime.as_ref(), display);
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::other(format!("desktop socket has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent)?;
    let mut parent_permissions = fs::metadata(parent)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut parent_permissions, 0o700);
    fs::set_permissions(parent, parent_permissions)?;
    remove_stale_socket(&path)?;

    let listener = UnixListener::bind(&path)?;
    let mut permissions = fs::metadata(&path)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o600);
    fs::set_permissions(&path, permissions)?;

    listener.set_nonblocking(true)?;
    let (sender, receiver) = channel::channel::<IpcMessage>();
    let requests = event_loop
        .handle()
        .insert_source(receiver, |event, _, state| {
            let envelope = match event {
                channel::Event::Msg(IpcMessage::Request(envelope)) => envelope,
                channel::Event::Msg(IpcMessage::Finished(id)) => {
                    let mut clients = state.ipc_server.clients.borrow_mut();
                    if let Some(index) = clients.iter().position(|client| client.id == id) {
                        let client = clients.swap_remove(index);
                        // The worker has finished socket I/O before sending completion.
                        let _ = client.thread.join();
                    }
                    return;
                }
                channel::Event::Closed => return,
            };
            {
                let reply = match envelope.request {
                    DesktopRequest::Subscribe { protocol } => {
                        if protocol.major != API_VERSION.major
                            || protocol.minor < 2
                            || protocol.minor > API_VERSION.minor
                        {
                            Reply::Response(DesktopResponse::Error(DesktopError {
                                code: DesktopErrorCode::IncompatibleVersion,
                                message: "Snapshot subscriptions require desktop API 1.2".into(),
                                retryable: false,
                            }))
                        } else if let Some(subscriber) = envelope.subscriber {
                            state.publish_desktop_state();
                            state.ipc_server.subscriptions.subscribe(&subscriber);
                            Reply::Subscribed(state.ipc_server.subscriptions.frame())
                        } else {
                            Reply::Response(invalid_request(
                                "Missing subscription transport".into(),
                            ))
                        }
                    }
                    request => Reply::Response(state.handle_ipc(request)),
                };
                let _ = envelope.response.send(reply);
            }
        })?;
    let clients = Rc::new(RefCell::new(Vec::<ClientWorker>::new()));
    let accept_clients = clients.clone();
    let mut next_client = 0u64;
    let listener_token = event_loop.handle().insert_source(
        Generic::new(listener, Interest::READ, Mode::Level),
        move |_, listener, _| {
            let mut clients = accept_clients.borrow_mut();
            let mut index = 0;
            while index < clients.len() {
                if clients[index].thread.is_finished() {
                    let _ = clients.swap_remove(index).thread.join();
                } else {
                    index += 1;
                }
            }
            // Limit work per callback so a connection flood cannot starve rendering.
            for _ in 0..MAX_CLIENTS {
                let (stream, _) = match listener.accept() {
                    Ok(client) => client,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        tracing::warn!(%error, "desktop IPC accept failed");
                        break;
                    }
                };
                if clients.len() >= MAX_CLIENTS {
                    continue;
                }
                stream.set_write_timeout(Some(IO_TIMEOUT))?;
                let control = stream.try_clone()?;
                let sender = sender.clone();
                let id = next_client;
                next_client = next_client.wrapping_add(1);
                match thread::Builder::new()
                    .name("knave-desktop-client".into())
                    .spawn(move || {
                        serve_connection(stream, sender.clone());
                        let _ = sender.send(IpcMessage::Finished(id));
                    }) {
                    Ok(thread) => clients.push(ClientWorker {
                        id,
                        socket: control,
                        thread,
                    }),
                    Err(error) => tracing::warn!(%error, "could not start desktop IPC client"),
                }
            }
            Ok(PostAction::Continue)
        },
    )?;
    tracing::info!(path = %path.display(), "Knave desktop IPC listening");
    Ok(IpcServer {
        path,
        subscriptions: Subscriptions::default(),
        clients,
        handle: event_loop.handle(),
        listener: listener_token,
        requests,
    })
}

fn serve_connection(
    mut stream: std::os::unix::net::UnixStream,
    sender: channel::Sender<IpcMessage>,
) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                if write_response(
                    &mut stream,
                    &invalid_request(format!("invalid request: {error}")),
                )
                .is_err()
                {
                    return;
                }
                continue;
            }
        };
        let subscription = if matches!(request, DesktopRequest::Subscribe { .. }) {
            match Subscriber::new() {
                Ok(subscription) => Some(subscription),
                Err(error) => {
                    tracing::warn!(%error, "could not create subscription wake socket");
                    return;
                }
            }
        } else {
            None
        };
        let (response, receive) = mpsc::channel();
        if sender
            .send(IpcMessage::Request(Envelope {
                request,
                response,
                subscriber: subscription.as_ref().map(|(s, _)| s.clone()),
            }))
            .is_err()
        {
            return;
        }
        match receive.recv_timeout(IO_TIMEOUT) {
            Ok(Reply::Response(response)) => {
                if write_response(&mut stream, &response).is_err() {
                    return;
                }
            }
            Ok(Reply::Subscribed(initial)) => {
                if stream.write_all(&initial).is_err() {
                    return;
                }
                if let Some((subscriber, mut wake)) = subscription {
                    // Reject pipelined requests buffered before switching to streaming mode.
                    if !reader.buffer().is_empty() {
                        return;
                    }
                    if let Err(error) = subscriptions::serve(&mut stream, &mut wake, &subscriber) {
                        tracing::debug!(%error, "desktop subscription ended");
                    }
                }
                return;
            }
            Err(_) => return,
        }
    }
}

fn write_response(
    stream: &mut std::os::unix::net::UnixStream,
    response: &DesktopResponse,
) -> Result<(), Box<dyn std::error::Error>> {
    serde_json::to_writer(&mut *stream, response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn invalid_request(message: String) -> DesktopResponse {
    DesktopResponse::Error(DesktopError {
        code: DesktopErrorCode::InvalidRequest,
        message,
        retryable: false,
    })
}

fn dispatch_error(error: DispatchError) -> DesktopResponse {
    let code = match error {
        DispatchError::UnknownWindow(_) | DispatchError::InvalidWorkspace(_) => {
            DesktopErrorCode::NotFound
        }
        DispatchError::NoFocusedWindow
        | DispatchError::NoMinimizedWindow
        | DispatchError::MinimizedWindow(_) => DesktopErrorCode::Unavailable,
        DispatchError::Config(_) | DispatchError::EmptyCommand | DispatchError::Spawn(_) => {
            DesktopErrorCode::Internal
        }
    };
    DesktopResponse::Error(DesktopError {
        code,
        message: error.to_string(),
        retryable: false,
    })
}

impl Villain {
    pub(crate) fn publish_desktop_state(&mut self) {
        if !self.desktop_state_dirty && self.ipc_server.subscriptions.snapshot.is_some() {
            return;
        }
        self.desktop_state_dirty = false;
        let snapshot = DesktopSnapshot {
            generation: 0,
            workspaces: self.workspace_info(),
            windows: self.window_info(),
        };
        self.ipc_server.subscriptions.publish(snapshot);
    }

    fn handle_ipc(&mut self, request: DesktopRequest) -> DesktopResponse {
        match request {
            DesktopRequest::Subscribe { .. } => {
                invalid_request("Subscribe requires a dedicated transport".into())
            }
            DesktopRequest::Dispatch(command) => match self.dispatch(Dispatch::from(command)) {
                Ok(()) => DesktopResponse::Ok,
                Err(error) => dispatch_error(error),
            },
            DesktopRequest::Query(query) => match query {
                DesktopQuery::Snapshot => {
                    self.publish_desktop_state();
                    DesktopResponse::Snapshot(
                        self.ipc_server
                            .subscriptions
                            .snapshot
                            .clone()
                            .expect("snapshot initialized"),
                    )
                }
                DesktopQuery::Windows => DesktopResponse::Windows(self.window_info()),
                DesktopQuery::Workspaces => DesktopResponse::Workspaces(self.workspace_info()),
                DesktopQuery::ActiveWindow => {
                    DesktopResponse::ActiveWindow(self.active_window_info())
                }
                DesktopQuery::ActiveWorkspace => DesktopResponse::ActiveWorkspace(WorkspaceId(
                    (self.active_workspace + 1) as u32,
                )),
                DesktopQuery::WorkspacePreview {
                    workspace,
                    width,
                    height,
                } => match crate::preview::capture(self, workspace.0 as usize, width, height) {
                    Ok(png) => DesktopResponse::WorkspacePreview(WorkspacePreview {
                        workspace,
                        width,
                        height,
                        png_base64: base64::engine::general_purpose::STANDARD.encode(png),
                    }),
                    Err(message) => DesktopResponse::Error(DesktopError {
                        code: DesktopErrorCode::Unavailable,
                        message,
                        retryable: true,
                    }),
                },
                DesktopQuery::Version => DesktopResponse::Version {
                    protocol: ProtocolVersion { ..API_VERSION },
                    component: format!("villain {}", env!("CARGO_PKG_VERSION")),
                },
            },
        }
    }
}

fn remove_stale_socket(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(path),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("refusing to replace non-socket {}", path.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_socket(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket()) {
        let _ = fs::remove_file(path);
    }
}
