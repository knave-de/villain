//! Session environment and desktop-portal activation.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    process::Command,
};

use crate::state::Villain;

const ACTIVATION_KEYS: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_DATA_DIRS",
];
/// Removes the Wayland socket owned by one Villain process on every exit path.
pub struct SocketCleanup {
    socket: PathBuf,
    lock: PathBuf,
}

impl SocketCleanup {
    pub fn new(socket_name: &OsStr) -> Option<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
        let socket = runtime.join(socket_name);
        let lock = PathBuf::from(format!("{}.lock", socket.display()));
        Some(Self { socket, lock })
    }
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let removed = match fs::symlink_metadata(&self.socket) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                fs::remove_file(&self.socket).is_ok()
            }
            Ok(_) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        };
        if removed {
            let _ = fs::remove_file(&self.lock);
        }
    }
}

/// Complete the environment inherited by compositor-launched applications.
pub fn prepare_environment(state: &mut Villain) {
    state.config.environment.insert(
        "WAYLAND_DISPLAY".into(),
        state.socket_name.to_string_lossy().into_owned(),
    );
    state.config.environment.remove("WAYLAND_SOCKET");
    if let Some(display) = state.xwayland_display {
        state
            .config
            .environment
            .insert("DISPLAY".into(), format!(":{display}"));
    } else {
        state.config.environment.remove("DISPLAY");
    }

    if let Some(data_dir) = villain_data_dir() {
        let current = state
            .config
            .environment
            .get("XDG_DATA_DIRS")
            .cloned()
            .or_else(|| std::env::var("XDG_DATA_DIRS").ok())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let data_dir = data_dir.to_string_lossy();
        if !current.split(':').any(|entry| entry == data_dir) {
            state
                .config
                .environment
                .insert("XDG_DATA_DIRS".into(), format!("{data_dir}:{current}"));
        }
    }
}

/// Publish Villain's environment to D-Bus/systemd activation.
///
/// This is only called for the TTY backend. A nested development compositor
/// must not replace the host desktop's activation environment.
pub fn activate(state: &Villain, restart_portal: bool) {
    let environment = activation_environment(state);

    // Session services are external processes and may wait indefinitely for a
    // backend to initialize. Never hold up the compositor event loop (or its
    // initial acquisition of DRM/input) while systemd or D-Bus is doing that
    // work.
    if let Err(error) = std::thread::Builder::new()
        .name("villain-session-activation".into())
        .spawn(move || activate_in_background(environment, restart_portal))
    {
        tracing::warn!(%error, "could not start session activation worker");
    }
}

fn activate_in_background(environment: BTreeMap<String, String>, restart_portal: bool) {
    run(
        Command::new("systemctl")
            .args(["--user", "import-environment"])
            .args(
                ACTIVATION_KEYS
                    .iter()
                    .filter(|key| environment.contains_key(**key)),
            )
            .envs(&environment),
        "import the Villain systemd activation environment",
    );

    let assignments = ACTIVATION_KEYS
        .iter()
        .filter_map(|key| environment.get(*key).map(|value| format!("{key}={value}")));
    run(
        Command::new("dbus-update-activation-environment")
            .arg("--systemd")
            .args(assignments)
            .envs(&environment),
        "import the Villain D-Bus activation environment",
    );

    if restart_portal {
        run(
            Command::new("systemctl")
                .args([
                    "--user",
                    "try-restart",
                    "--no-block",
                    "xdg-desktop-portal-gtk.service",
                ])
                .envs(&environment),
            "restart the GTK portal backend for Villain",
        );
        run(
            Command::new("systemctl")
                .args([
                    "--user",
                    "try-restart",
                    "--no-block",
                    "xdg-desktop-portal.service",
                ])
                .envs(&environment),
            "restart the desktop portal frontend for Villain",
        );
    }
}

fn activation_environment(state: &Villain) -> BTreeMap<String, String> {
    let mut environment = state.config.environment.clone();
    if let Some(display) = state.xwayland_display {
        environment.insert("DISPLAY".into(), format!(":{display}"));
    } else {
        // An empty value replaces a stale host DISPLAY in both activation
        // environments until Villain's own XWayland is ready.
        environment.insert("DISPLAY".into(), String::new());
    }
    environment
}

fn run(command: &mut Command, purpose: &str) {
    match command.status() {
        Ok(status) if status.success() => {
            tracing::debug!(%purpose, "session integration succeeded")
        }
        Ok(status) => tracing::warn!(%status, %purpose, "session integration command failed"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(%purpose, "session integration command is not installed")
        }
        Err(error) => tracing::warn!(%error, %purpose, "could not run session integration command"),
    }
}

fn villain_data_dir() -> Option<PathBuf> {
    if let Some(path) = option_env!("VILLAIN_DATA_DIR") {
        let path = PathBuf::from(path);
        if has_portal_config(&path) {
            return Some(path);
        }
    }

    if let Ok(executable) = std::env::current_exe()
        && let Some(prefix) = executable.parent().and_then(Path::parent)
    {
        let path = prefix.join("share");
        if has_portal_config(&path) {
            return Some(path);
        }
    }

    if cfg!(debug_assertions) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("share");
        if has_portal_config(&path) {
            return Some(path);
        }
    }
    None
}

fn has_portal_config(path: &Path) -> bool {
    path.join("xdg-desktop-portal/villain-portals.conf")
        .is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_portal_configuration_selects_gtk() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("share");
        assert!(has_portal_config(&path));
        let contents =
            std::fs::read_to_string(path.join("xdg-desktop-portal/villain-portals.conf")).unwrap();
        assert!(contents.contains("[preferred]"));
        assert!(contents.contains("default=gtk"));
    }
}
