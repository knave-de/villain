//! Bounded standard output capture. Capture-only timers exist while frames are pending.
use crate::state::Villain;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, Offscreen, Renderer, TextureMapping,
            damage::OutputDamageTracker,
            gles::{GlesRenderbuffer, GlesRenderer},
        },
    },
    desktop::space::render_output,
    output::Output,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            protocol::{wl_buffer::WlBuffer, wl_output, wl_shm},
        },
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
    wayland::shm::with_buffer_contents_mut,
};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use wayland_protocols::ext::{
    image_capture_source::v1::server::{
        ext_image_capture_source_v1 as source,
        ext_output_image_capture_source_manager_v1 as sources,
    },
    image_copy_capture::v1::server::{
        ext_image_copy_capture_cursor_session_v1 as cursor,
        ext_image_copy_capture_frame_v1 as frame, ext_image_copy_capture_manager_v1 as manager,
        ext_image_copy_capture_session_v1 as session,
    },
};
const MAX_SESSIONS: usize = 8;
const MAX_PIXELS: i64 = 8 * 1024 * 1024;
#[derive(Default)]
pub struct CaptureState {
    sessions: Vec<Weak<SessionData>>,
    pending: Vec<frame::ExtImageCopyCaptureFrameV1>,
    scheduled: bool,
}
pub struct SourceData(Option<Output>);
pub struct SessionData {
    output: Option<Output>,
    cursor: bool,
    size: Mutex<Size<i32, Buffer>>,
    active: Mutex<bool>,
    alive: Mutex<bool>,
}
pub struct FrameData {
    session: Arc<SessionData>,
    session_resource: session::ExtImageCopyCaptureSessionV1,
    buffer: Mutex<Option<WlBuffer>>,
    captured: Mutex<bool>,
}
pub fn init(display: &DisplayHandle) {
    display.create_global::<Villain, sources::ExtOutputImageCaptureSourceManagerV1, _>(1, ());
    display.create_global::<Villain, manager::ExtImageCopyCaptureManagerV1, _>(1, ());
}
fn constraints(resource: &session::ExtImageCopyCaptureSessionV1, data: &SessionData) {
    if data.output.is_none() {
        resource.stopped();
        return;
    }
    let size = *data.size.lock().unwrap();
    resource.buffer_size(size.w as u32, size.h as u32);
    resource.shm_format(wl_shm::Format::Abgr8888);
    resource.done();
}
impl GlobalDispatch<sources::ExtOutputImageCaptureSourceManagerV1, ()> for Villain {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<sources::ExtOutputImageCaptureSourceManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}
impl GlobalDispatch<manager::ExtImageCopyCaptureManagerV1, ()> for Villain {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<manager::ExtImageCopyCaptureManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}
impl Dispatch<sources::ExtOutputImageCaptureSourceManagerV1, ()> for Villain {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &sources::ExtOutputImageCaptureSourceManagerV1,
        request: sources::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let sources::Request::CreateSource { source, output } = request {
            init.init(source, SourceData(Output::from_resource(&output)));
        }
    }
}
impl Dispatch<source::ExtImageCaptureSourceV1, SourceData> for Villain {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &source::ExtImageCaptureSourceV1,
        _: source::Request,
        _: &SourceData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}
impl Dispatch<manager::ExtImageCopyCaptureManagerV1, ()> for Villain {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &manager::ExtImageCopyCaptureManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            manager::Request::CreateSession {
                session,
                source,
                options,
            } => {
                let bits = u32::from(options);
                if bits & !1 != 0 {
                    resource.post_error(manager::Error::InvalidOption, "unknown capture option");
                }
                state.capture.sessions.retain(|s| s.strong_count() > 0);
                let output = source.data::<SourceData>().and_then(|s| s.0.clone());
                let size = output
                    .as_ref()
                    .and_then(Output::current_mode)
                    .map(|m| Size::<i32, Buffer>::from((m.size.w, m.size.h)))
                    .unwrap_or_default();
                let valid = state.capture.sessions.len() < MAX_SESSIONS
                    && size.w > 0
                    && size.h > 0
                    && i64::from(size.w) * i64::from(size.h) <= MAX_PIXELS;
                let data = Arc::new(SessionData {
                    output: output.filter(|_| valid),
                    cursor: bits & 1 != 0,
                    size: Mutex::new(size),
                    active: Mutex::new(false),
                    alive: Mutex::new(valid),
                });
                let session = init.init(session, Arc::clone(&data));
                constraints(&session, &data);
                if valid {
                    state.capture.sessions.push(Arc::downgrade(&data));
                }
            }
            manager::Request::CreatePointerCursorSession { session, .. } => {
                init.init(session, ());
            }
            _ => {}
        }
    }
}
impl Dispatch<cursor::ExtImageCopyCaptureCursorSessionV1, ()> for Villain {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &cursor::ExtImageCopyCaptureCursorSessionV1,
        request: cursor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let cursor::Request::GetCaptureSession { session } = request {
            let data = Arc::new(SessionData {
                output: None,
                cursor: false,
                size: Mutex::new(Size::default()),
                active: Mutex::new(false),
                alive: Mutex::new(false),
            });
            init.init(session, data).stopped();
        }
    }
}
impl Dispatch<session::ExtImageCopyCaptureSessionV1, Arc<SessionData>> for Villain {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &session::ExtImageCopyCaptureSessionV1,
        request: session::Request,
        data: &Arc<SessionData>,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            session::Request::CreateFrame { frame } => {
                let mut active = data.active.lock().unwrap();
                if *active {
                    resource.post_error(session::Error::DuplicateFrame, "one frame per session");
                }
                *active = true;
                init.init(
                    frame,
                    FrameData {
                        session: Arc::clone(data),
                        session_resource: resource.clone(),
                        buffer: Mutex::new(None),
                        captured: Mutex::new(false),
                    },
                );
            }
            session::Request::Destroy => {
                *data.alive.lock().unwrap() = false;
            }
            _ => {}
        }
    }
    fn destroyed(
        _: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        _: &session::ExtImageCopyCaptureSessionV1,
        data: &Arc<SessionData>,
    ) {
        *data.alive.lock().unwrap() = false;
    }
}
impl Dispatch<frame::ExtImageCopyCaptureFrameV1, FrameData> for Villain {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &frame::ExtImageCopyCaptureFrameV1,
        request: frame::Request,
        data: &FrameData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if *data.captured.lock().unwrap() && !matches!(request, frame::Request::Destroy) {
            resource.post_error(frame::Error::AlreadyCaptured, "capture already submitted");
            return;
        }
        match request {
            frame::Request::AttachBuffer { buffer } => {
                *data.buffer.lock().unwrap() = Some(buffer);
            }
            frame::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => {
                if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    resource.post_error(
                        frame::Error::InvalidBufferDamage,
                        "invalid damage rectangle",
                    );
                }
            }
            frame::Request::Capture => {
                *data.captured.lock().unwrap() = true;
                if data.buffer.lock().unwrap().is_none() {
                    resource.post_error(frame::Error::NoBuffer, "attach a buffer before capture");
                    return;
                }
                if !*data.session.alive.lock().unwrap() {
                    resource.failed(frame::FailureReason::Stopped);
                    return;
                }
                if state.capture.pending.len() >= MAX_SESSIONS {
                    resource.failed(frame::FailureReason::Unknown);
                    return;
                }
                state.capture.pending.push(resource.clone());
                if !state.capture.scheduled {
                    state.capture.scheduled = true;
                    if let Err(error) = state.loop_handle.insert_source(
                        Timer::from_duration(Duration::from_millis(33)),
                        |_, _, state| {
                            finish_pending(state);
                            TimeoutAction::Drop
                        },
                    ) {
                        tracing::warn!(%error, "could not schedule capture");
                        fail_pending(state);
                    }
                }
            }
            _ => {}
        }
    }
    fn destroyed(
        _: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        _: &frame::ExtImageCopyCaptureFrameV1,
        data: &FrameData,
    ) {
        *data.session.active.lock().unwrap() = false;
    }
}
fn fail_pending(state: &mut Villain) {
    state.capture.scheduled = false;
    for frame in state.capture.pending.drain(..) {
        frame.failed(frame::FailureReason::Unknown);
    }
}
fn finish_pending(state: &mut Villain) {
    state.capture.scheduled = false;
    for frame in std::mem::take(&mut state.capture.pending) {
        if !frame.is_alive() {
            continue;
        }
        let data = frame.data::<FrameData>().unwrap();
        let Some(output) = &data.session.output else {
            frame.failed(frame::FailureReason::Stopped);
            continue;
        };
        let mapped = state.space.outputs().any(|o| o == output);
        let size = *data.session.size.lock().unwrap();
        if !mapped {
            *data.session.alive.lock().unwrap() = false;
            data.session_resource.stopped();
        }
        if !mapped || !*data.session.alive.lock().unwrap() {
            frame.failed(frame::FailureReason::Stopped);
            continue;
        }
        if let Some(mode) = output.current_mode() {
            if mode.size.w != size.w || mode.size.h != size.h {
                if i64::from(mode.size.w) * i64::from(mode.size.h) > MAX_PIXELS {
                    *data.session.alive.lock().unwrap() = false;
                    data.session_resource.stopped();
                    frame.failed(frame::FailureReason::Stopped);
                } else {
                    *data.session.size.lock().unwrap() = (mode.size.w, mode.size.h).into();
                    constraints(&data.session_resource, &data.session);
                    frame.failed(frame::FailureReason::BufferConstraints);
                }
                continue;
            }
        } else {
            data.session_resource.stopped();
            frame.failed(frame::FailureReason::Stopped);
            continue;
        }
        let Some(buffer) = data.buffer.lock().unwrap().clone() else {
            frame.failed(frame::FailureReason::BufferConstraints);
            continue;
        };
        let pixels = render(state, output, data.session.cursor, size);
        let Ok(pixels) = pixels else {
            frame.failed(frame::FailureReason::Unknown);
            continue;
        };
        let valid = with_buffer_contents_mut(&buffer, |ptr, len, info| {
            if info.format != wl_shm::Format::Abgr8888
                || info.width != size.w
                || info.height != size.h
                || info.stride < size.w * 4
                || info.offset < 0
            {
                return false;
            }
            let stride = info.stride as usize;
            let row = size.w as usize * 4;
            let end = (info.offset as usize)
                .checked_add(stride * (size.h as usize - 1))
                .and_then(|n| n.checked_add(row));
            if end.is_none_or(|end| end > len) {
                return false;
            }
            for y in 0..size.h as usize {
                // SAFETY: validated pool bounds, format, stride and source size. No references to shared memory escape.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        pixels.as_ptr().add(y * row),
                        ptr.add(info.offset as usize + y * stride),
                        row,
                    );
                }
            }
            true
        })
        .unwrap_or(false);
        if !valid {
            frame.failed(frame::FailureReason::BufferConstraints);
            continue;
        }
        frame.transform(wl_output::Transform::Normal);
        frame.damage(0, 0, size.w, size.h);
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: ts is a valid writable timespec.
        unsafe {
            libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
        }
        frame.presentation_time(
            (ts.tv_sec as u64 >> 32) as u32,
            ts.tv_sec as u32,
            ts.tv_nsec as u32,
        );
        frame.ready();
    }
    let _ = state.display_handle.flush_clients();
}
fn render(
    state: &mut Villain,
    output: &Output,
    cursor: bool,
    size: Size<i32, Buffer>,
) -> Result<Vec<u8>, String> {
    if let Some(mut tty) = state.tty.take() {
        let result = if tty.is_active() {
            render_with(state, &mut tty.renderer, output, cursor, size)
        } else {
            Err("session inactive".into())
        };
        state.tty = Some(tty);
        result
    } else if let Some(mut winit) = state.winit.take() {
        let result = render_with(state, winit.renderer(), output, cursor, size);
        state.winit = Some(winit);
        result
    } else {
        Err("no renderer".into())
    }
}
fn render_with(
    state: &mut Villain,
    renderer: &mut GlesRenderer,
    output: &Output,
    cursor: bool,
    size: Size<i32, Buffer>,
) -> Result<Vec<u8>, String> {
    let mut target = Offscreen::<GlesRenderbuffer>::create_buffer(renderer, Fourcc::Abgr8888, size)
        .map_err(|e| e.to_string())?;
    let mut framebuffer = renderer.bind(&mut target).map_err(|e| e.to_string())?;
    let cursors = if cursor {
        state
            .cursor
            .render_elements(renderer, state.pointer_location, state.start_time.elapsed())
    } else {
        Vec::new()
    };
    let elements = crate::overview_render::elements(state, renderer, cursors);
    let mut damage = OutputDamageTracker::new(
        Size::<i32, Physical>::from((size.w, size.h)),
        output.current_scale().fractional_scale(),
        Transform::Normal,
    );
    let result = render_output(
        output,
        renderer,
        &mut framebuffer,
        1.0,
        0,
        [&state.space],
        &elements,
        &mut damage,
        [0.08, 0.05, 0.12, 1.0],
    )
    .map_err(|e| format!("{e:?}"))?;
    result.sync.wait().map_err(|e| e.to_string())?;
    let mapping = renderer
        .copy_framebuffer(&framebuffer, Rectangle::from_size(size), Fourcc::Abgr8888)
        .map_err(|e| e.to_string())?;
    let flipped = mapping.flipped();
    let mut pixels = renderer
        .map_texture(&mapping)
        .map_err(|e| e.to_string())?
        .to_vec();
    // TextureMapping::flipped is relative to GL lower-left; PNG/SHM use top-left.
    if !flipped {
        let row = size.w as usize * 4;
        for y in 0..size.h as usize / 2 {
            let (top, bottom) = pixels.split_at_mut((size.h as usize - 1 - y) * row);
            top[y * row..(y + 1) * row].swap_with_slice(&mut bottom[..row]);
        }
    }
    // Offscreen-only capture does not submit a Winit/TTY framebuffer, so drain
    // renderer destruction callbacks here instead of retaining each readback PBO.
    drop(mapping);
    drop(framebuffer);
    drop(target);
    renderer
        .cleanup_texture_cache()
        .map_err(|e| e.to_string())?;
    Ok(pixels)
}
