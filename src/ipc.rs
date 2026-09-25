//! JSON-lines server for Knave’s versioned desktop contract.

use std::{
    ffi::OsStr,
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{fs::FileTypeExt, net::UnixListener},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use base64::Engine;
use knave_desktop_api::{
    API_VERSION, DesktopError, DesktopErrorCode, DesktopQuery, DesktopRequest, DesktopResponse,
    DesktopSnapshot, ProtocolVersion, WorkspaceId, WorkspacePreview, socket_path_for_display,
};
use smithay::reexports::calloop::{EventLoop, channel};

use crate::{
    dispatch::{Dispatch, DispatchError},
    state::Villain,
};

struct Envelope {
    request: DesktopRequest,
    response: mpsc::Sender<DesktopResponse>,
}

pub struct IpcServer {
    path: PathBuf,
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        remove_socket(&self.path);
    }
}

pub fn init(
    event_loop: &mut EventLoop<Villain>,
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

    let (sender, receiver) = channel::channel::<Envelope>();
    event_loop
        .handle()
        .insert_source(receiver, |event, _, state| {
            if let channel::Event::Msg(envelope) = event {
                let response = state.handle_ipc(envelope.request);
                let _ = envelope.response.send(response);
            }
        })?;

    thread::Builder::new()
        .name("knave-desktop-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    break;
                };
                let sender = sender.clone();
                let _ = thread::Builder::new()
                    .name("knave-desktop-client".into())
                    .spawn(move || serve_connection(stream, sender));
            }
        })?;

    tracing::info!(path = %path.display(), "Knave desktop IPC listening");
    Ok(IpcServer { path })
}

fn serve_connection(mut stream: std::os::unix::net::UnixStream, sender: channel::Sender<Envelope>) {
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
                let response = invalid_request(format!("invalid request: {error}"));
                if write_response(&mut stream, &response).is_err() {
                    return;
                }
                continue;
            }
        };
        let (response_sender, response_receiver) = mpsc::channel();
        if sender
            .send(Envelope {
                request,
                response: response_sender,
            })
            .is_err()
        {
            return;
        }
        let Ok(response) = response_receiver.recv() else {
            return;
        };
        if write_response(&mut stream, &response).is_err() {
            return;
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
    fn handle_ipc(&mut self, request: DesktopRequest) -> DesktopResponse {
        match request {
            DesktopRequest::Dispatch(command) => match self.dispatch(Dispatch::from(command)) {
                Ok(()) => DesktopResponse::Ok,
                Err(error) => dispatch_error(error),
            },
            DesktopRequest::Query(query) => match query {
                DesktopQuery::Snapshot => DesktopResponse::Snapshot(DesktopSnapshot {
                    generation: self.next_window_id,
                    workspaces: self.workspace_info(),
                    windows: self.window_info(),
                }),
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
