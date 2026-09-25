//! Central imperative command interface for compositor state changes.

use std::{fmt, io};

use knave_desktop_api::{DesktopCommand, WindowId};

use crate::state::Villain;

#[derive(Clone, Debug, PartialEq)]
pub enum Dispatch {
    ReloadConfig,
    CloseFocused,
    MinimizeFocused,
    RestoreLastMinimized,
    FocusWorkspace(usize),
    PreviousWorkspace,
    NextWorkspace,
    FocusWindow(WindowId),
    RestoreWindow(WindowId),
    Spawn(Vec<String>),
    Quit,
}

impl From<DesktopCommand> for Dispatch {
    fn from(request: DesktopCommand) -> Self {
        match request {
            DesktopCommand::ReloadConfiguration => Self::ReloadConfig,
            DesktopCommand::CloseFocused => Self::CloseFocused,
            DesktopCommand::MinimizeFocused => Self::MinimizeFocused,
            DesktopCommand::RestoreLastMinimized => Self::RestoreLastMinimized,
            DesktopCommand::FocusWorkspace { workspace } => {
                Self::FocusWorkspace(workspace.0 as usize)
            }
            DesktopCommand::FocusWindow { window } => Self::FocusWindow(window),
            DesktopCommand::RestoreWindow { window } => Self::RestoreWindow(window),
            DesktopCommand::Spawn { argv } => Self::Spawn(argv),
            DesktopCommand::Quit => Self::Quit,
        }
    }
}

#[derive(Debug)]
pub enum DispatchError {
    Config(String),
    NoFocusedWindow,
    NoMinimizedWindow,
    InvalidWorkspace(usize),
    UnknownWindow(WindowId),
    MinimizedWindow(WindowId),
    EmptyCommand,
    Spawn(io::Error),
}

impl fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "configuration reload failed: {error}"),
            Self::NoFocusedWindow => write!(formatter, "no window is focused"),
            Self::NoMinimizedWindow => write!(formatter, "no minimized window to restore"),
            Self::InvalidWorkspace(workspace) => {
                write!(formatter, "workspace {workspace} does not exist")
            }
            Self::UnknownWindow(window) => write!(formatter, "window {} does not exist", window.0),
            Self::MinimizedWindow(window) => {
                write!(
                    formatter,
                    "window {} is minimized; restore it first",
                    window.0
                )
            }
            Self::EmptyCommand => write!(formatter, "spawn command is empty"),
            Self::Spawn(error) => write!(formatter, "could not spawn command: {error}"),
        }
    }
}

impl std::error::Error for DispatchError {}

impl Villain {
    pub fn dispatch(&mut self, dispatch: Dispatch) -> Result<(), DispatchError> {
        match dispatch {
            Dispatch::ReloadConfig => {
                let config = self
                    .config
                    .reload()
                    .map_err(|error| DispatchError::Config(error.to_string()))?;
                self.config = config;
                crate::session::prepare_environment(self);
                if self.owns_session {
                    crate::session::activate(self, false);
                }
                self.apply_input_config();
                tracing::info!("configuration reloaded");
                Ok(())
            }
            Dispatch::CloseFocused => self
                .close_focused_window()
                .then_some(())
                .ok_or(DispatchError::NoFocusedWindow),
            Dispatch::MinimizeFocused => self
                .minimize_focused_window()
                .then_some(())
                .ok_or(DispatchError::NoFocusedWindow),
            Dispatch::RestoreLastMinimized => self
                .restore_last_minimized_window()
                .then_some(())
                .ok_or(DispatchError::NoMinimizedWindow),
            Dispatch::FocusWorkspace(workspace) => {
                if !(1..=self.workspaces.len()).contains(&workspace) {
                    return Err(DispatchError::InvalidWorkspace(workspace));
                }
                self.switch_workspace(workspace - 1);
                Ok(())
            }
            Dispatch::PreviousWorkspace => {
                self.switch_workspace(
                    (self.active_workspace + self.workspaces.len() - 1) % self.workspaces.len(),
                );
                Ok(())
            }
            Dispatch::NextWorkspace => {
                self.switch_workspace((self.active_workspace + 1) % self.workspaces.len());
                Ok(())
            }
            Dispatch::FocusWindow(window) => match self.focus_window(window) {
                Some(true) => Ok(()),
                Some(false) => Err(DispatchError::MinimizedWindow(window)),
                None => Err(DispatchError::UnknownWindow(window)),
            },
            Dispatch::RestoreWindow(window) => self
                .restore_window(window)
                .then_some(())
                .ok_or(DispatchError::UnknownWindow(window)),
            Dispatch::Spawn(argv) => {
                if argv.is_empty() {
                    Err(DispatchError::EmptyCommand)
                } else {
                    self.spawn(argv).map_err(DispatchError::Spawn)
                }
            }
            Dispatch::Quit => {
                self.loop_signal.stop();
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_dispatch_maps_to_internal_dispatch() {
        assert_eq!(
            Dispatch::from(DesktopCommand::FocusWorkspace {
                workspace: knave_desktop_api::WorkspaceId(2),
            }),
            Dispatch::FocusWorkspace(2)
        );
        assert_eq!(
            Dispatch::from(DesktopCommand::MinimizeFocused),
            Dispatch::MinimizeFocused
        );
    }
}
