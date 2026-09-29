//! Central imperative command interface for compositor state changes.

use std::{fmt, io};

use knave_desktop_api::{DesktopCommand, WindowId, WorkspaceId};

use crate::state::Villain;

#[derive(Clone, Debug, PartialEq)]
pub enum Dispatch {
    ReloadConfig,
    ResizeMaster(i16),
    ResetMaster,
    CloseFocused,
    MinimizeFocused,
    MaximizeFocused,
    UnmaximizeFocused,
    ToggleMaximizeFocused,
    RestoreLastMinimized,
    FocusWorkspace(usize),
    PreviousWorkspace,
    NextWorkspace,
    FocusWindow(WindowId),
    RestoreWindow(WindowId),
    RestoreAndFocusWindow(WindowId),
    FocusOverviewPoint {
        workspace: WorkspaceId,
        x: i32,
        y: i32,
    },
    ToggleOverview,
    Spawn(Vec<String>),
    Quit,
}

impl From<DesktopCommand> for Dispatch {
    fn from(request: DesktopCommand) -> Self {
        match request {
            DesktopCommand::ReloadConfiguration => Self::ReloadConfig,
            DesktopCommand::CloseFocused => Self::CloseFocused,
            DesktopCommand::MaximizeFocused => Self::MaximizeFocused,
            DesktopCommand::UnmaximizeFocused => Self::UnmaximizeFocused,
            DesktopCommand::ToggleMaximizeFocused => Self::ToggleMaximizeFocused,
            DesktopCommand::MinimizeFocused => Self::MinimizeFocused,
            DesktopCommand::RestoreLastMinimized => Self::RestoreLastMinimized,
            DesktopCommand::FocusWorkspace { workspace } => {
                Self::FocusWorkspace(workspace.0 as usize)
            }
            DesktopCommand::FocusWindow { window } => Self::FocusWindow(window),
            DesktopCommand::RestoreWindow { window } => Self::RestoreWindow(window),
            DesktopCommand::RestoreAndFocusWindow { window } => Self::RestoreAndFocusWindow(window),
            DesktopCommand::FocusOverviewPoint { workspace, x, y } => {
                Self::FocusOverviewPoint { workspace, x, y }
            }
            DesktopCommand::ToggleOverview => Self::ToggleOverview,
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
    InvalidOverviewPoint,
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
            Self::InvalidOverviewPoint => {
                write!(formatter, "point is outside the active overview pane")
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
                self.release_pointer_buttons();
                self.config = config;
                self.relayout_active_workspace();
                self.request_repaint();
                crate::session::prepare_environment(self);
                if self.owns_session {
                    crate::session::activate(self, false);
                }
                self.apply_input_config();
                tracing::info!("configuration reloaded");
                Ok(())
            }
            Dispatch::ResizeMaster(delta) => {
                self.resize_master(Some(delta));
                Ok(())
            }
            Dispatch::ResetMaster => {
                self.resize_master(None);
                Ok(())
            }
            Dispatch::CloseFocused => self
                .close_focused_window()
                .then_some(())
                .ok_or(DispatchError::NoFocusedWindow),
            Dispatch::MaximizeFocused => self
                .maximize_focused_window(Some(true))
                .then_some(())
                .ok_or(DispatchError::NoFocusedWindow),
            Dispatch::UnmaximizeFocused => self
                .maximize_focused_window(Some(false))
                .then_some(())
                .ok_or(DispatchError::NoFocusedWindow),
            Dispatch::ToggleMaximizeFocused => self
                .maximize_focused_window(None)
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
            Dispatch::RestoreAndFocusWindow(window) => {
                if !self.restore_window(window) {
                    return Err(DispatchError::UnknownWindow(window));
                }
                self.focus_window(window)
                    .and_then(|focused| focused.then_some(()))
                    .ok_or(DispatchError::MinimizedWindow(window))
            }
            Dispatch::FocusOverviewPoint { workspace, x, y } => {
                let index = workspace.0 as usize;
                if index == 0 || index > self.workspaces.len() {
                    return Err(DispatchError::InvalidWorkspace(index));
                }
                let Some(window) = self.overview_window_at(workspace, x, y) else {
                    return Err(DispatchError::InvalidOverviewPoint);
                };
                if let Some(window) = window {
                    self.focus_window(window)
                        .and_then(|focused| focused.then_some(()))
                        .ok_or(DispatchError::UnknownWindow(window))
                } else {
                    self.switch_workspace(index - 1);
                    Ok(())
                }
            }
            Dispatch::ToggleOverview => self.toggle_overview().map_err(DispatchError::Spawn),
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
