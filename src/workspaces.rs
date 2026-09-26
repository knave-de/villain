//! Ordered workspaces with a master-and-stack layout.
use crate::{focus::KeyboardFocus, state::Villain};
use knave_desktop_api::{
    WindowId, WindowSummary as WindowInfo, WorkspaceId, WorkspaceSummary as WorkspaceInfo,
};
use smithay::{
    desktop::{Window, WindowSurfaceType},
    input::pointer::MotionEvent,
    reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::Resource},
    utils::{Point, Rectangle, SERIAL_COUNTER, Size},
    wayland::{seat::WaylandFocus, shell::xdg::ToplevelSurface},
    xwayland::X11Surface,
};
use std::process::Command;

#[derive(Default)]
pub struct Workspace {
    windows: Vec<WorkspaceWindow>,
    /// Most-recently-focused managed windows, oldest first.
    focus_history: Vec<WindowId>,
    minimized_history: Vec<Window>,
    fullscreen: Option<WindowId>,
}

struct WorkspaceWindow {
    id: WindowId,
    window: Window,
    minimized: bool,
    parent: Option<WindowId>,
    floating: Option<Rectangle<i32, smithay::utils::Logical>>,
}

#[derive(Clone, Copy)]
pub(crate) enum WindowAction {
    Close,
    Minimize,
}

fn master_stack_layout(
    output: Size<i32, smithay::utils::Logical>,
    window_count: usize,
) -> Vec<(
    Point<i32, smithay::utils::Logical>,
    Size<i32, smithay::utils::Logical>,
)> {
    match window_count {
        0 => Vec::new(),
        1 => vec![((0, 0).into(), output)],
        _ => {
            let master_width = output.w / 2;
            let stack_width = output.w - master_width;
            let stack_count = window_count as i32 - 1;
            let stack_height = output.h / stack_count;
            let mut result = Vec::with_capacity(window_count);
            result.push(((0, 0).into(), (master_width, output.h).into()));
            for index in 0..stack_count {
                let y = index * stack_height;
                let height = if index + 1 == stack_count {
                    output.h - y
                } else {
                    stack_height
                };
                result.push(((master_width, y).into(), (stack_width, height).into()));
            }
            result
        }
    }
}

type Geometry = Rectangle<i32, smithay::utils::Logical>;
type WindowPlacement = (Window, Geometry, bool, bool, bool);

pub(crate) struct WorkspacePreviewScene {
    pub output_size: Size<i32, smithay::utils::Logical>,
    pub windows: Vec<(Window, Geometry)>,
}

fn fixed_size(
    min: Size<i32, smithay::utils::Logical>,
    max: Size<i32, smithay::utils::Logical>,
) -> bool {
    min.w > 0 && min.h > 0 && min == max
}

pub(crate) fn constrain_geometry(
    mut rect: Geometry,
    output: Size<i32, smithay::utils::Logical>,
    min: Size<i32, smithay::utils::Logical>,
    max: Size<i32, smithay::utils::Logical>,
) -> Geometry {
    let limit = |size: i32, min: i32, max: i32, output: i32| {
        let upper = if max > 0 { max.min(output) } else { output }.max(1);
        size.clamp(min.max(1).min(upper), upper)
    };
    rect.size.w = limit(rect.size.w, min.w, max.w, output.w);
    rect.size.h = limit(rect.size.h, min.h, max.h, output.h);
    rect.loc.x = rect.loc.x.clamp(0, (output.w - rect.size.w).max(0));
    rect.loc.y = rect.loc.y.clamp(0, (output.h - rect.size.h).max(0));
    rect
}

impl Workspace {
    fn descendant_of(&self, entry: &WorkspaceWindow, ancestor: WindowId) -> bool {
        let mut parent = entry.parent;
        for _ in 0..self.windows.len() {
            let Some(id) = parent else {
                break;
            };
            if id == ancestor {
                return true;
            }
            parent = self
                .windows
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| entry.parent);
        }
        false
    }

    fn hidden_by_parent(&self, entry: &WorkspaceWindow) -> bool {
        self.windows
            .iter()
            .any(|parent| parent.minimized && self.descendant_of(entry, parent.id))
    }
}

impl Villain {
    pub(crate) fn window_constraints(
        window: &Window,
    ) -> (
        Size<i32, smithay::utils::Logical>,
        Size<i32, smithay::utils::Logical>,
    ) {
        if let Some(surface) = window.toplevel() {
            smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
                let mut guard = states
                    .cached_state
                    .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
                let state = guard.current();
                (state.min_size, state.max_size)
            })
        } else if let Some(surface) = window.x11_surface() {
            (
                surface.min_size().unwrap_or_default(),
                surface.max_size().unwrap_or_default(),
            )
        } else {
            (Size::default(), Size::default())
        }
    }

    /// Apply client hints on map and when committed hints/parent relationships change.
    pub fn refresh_window_hints(&mut self, window: &Window) {
        let parent_window = if let Some(surface) = window.toplevel() {
            surface
                .parent()
                .and_then(|surface| self.window_for_surface(&surface))
        } else {
            window
                .x11_surface()
                .and_then(|surface| surface.is_transient_for())
                .and_then(|id| self.window_for_x11_id(id))
        }
        .filter(|parent| parent != window);
        let parent = parent_window.as_ref().and_then(|parent| {
            self.workspaces
                .iter()
                .enumerate()
                .find_map(|(index, workspace)| {
                    workspace
                        .windows
                        .iter()
                        .find(|entry| entry.window == *parent)
                        .map(|entry| (index, entry.id))
                })
        });
        let Some((mut index, position)) =
            self.workspaces
                .iter()
                .enumerate()
                .find_map(|(index, workspace)| {
                    workspace
                        .windows
                        .iter()
                        .position(|entry| entry.window == *window)
                        .map(|pos| (index, pos))
                })
        else {
            return;
        };
        let mut changed = false;
        let mut moved_from = None;
        if let Some((target, _)) = parent
            && index != target
        {
            let entry = self.workspaces[index].windows.remove(position);
            self.workspaces[index]
                .minimized_history
                .retain(|candidate| candidate != window);
            if self.workspaces[index].fullscreen == Some(entry.id) {
                self.workspaces[index].fullscreen = None;
                self.workspaces[target].fullscreen = Some(entry.id);
            }
            if entry.minimized {
                self.workspaces[target]
                    .minimized_history
                    .push(window.clone());
            }
            self.workspaces[target].windows.push(entry);
            moved_from = Some(index);
            index = target;
            changed = true;
        }
        let (min, max) = Self::window_constraints(window);
        let floating = parent.is_some()
            || fixed_size(min, max)
            || window.x11_surface().is_some_and(|surface| {
                surface.window_type() == Some(smithay::xwayland::xwm::WmWindowType::Dialog)
            });
        let parent_rect = parent_window
            .as_ref()
            .and_then(|parent| {
                self.workspace_layout(index)
                    .into_iter()
                    .find(|(window, _, _, _, _)| window == parent)
                    .map(|(_, rect, _, _, _)| rect)
            })
            .unwrap_or_else(|| Rectangle::from_size(self.output_size));
        let entry = self.workspaces[index]
            .windows
            .iter_mut()
            .find(|entry| entry.window == *window)
            .unwrap();
        let parent_id = parent.map(|(_, id)| id);
        changed |= entry.parent != parent_id;
        entry.parent = parent_id;
        let geometry = if floating {
            let geometry = entry.floating.unwrap_or_else(|| {
                let natural = window.geometry().size;
                let size = if fixed_size(min, max) {
                    min
                } else if natural.w > 0 && natural.h > 0 {
                    natural
                } else {
                    (600, 400).into()
                };
                let size =
                    constrain_geometry(Rectangle::from_size(size), self.output_size, min, max).size;
                Rectangle::new(
                    (
                        parent_rect.loc.x + (parent_rect.size.w - size.w) / 2,
                        parent_rect.loc.y + (parent_rect.size.h - size.h) / 2,
                    )
                        .into(),
                    size,
                )
            });
            Some(constrain_geometry(geometry, self.output_size, min, max))
        } else {
            None
        };
        changed |= entry.floating != geometry;
        entry.floating = geometry;
        if changed {
            if index == self.active_workspace || moved_from == Some(self.active_workspace) {
                self.relayout_active_workspace();
            }
            if index != self.active_workspace {
                self.configure_workspace(index);
            }
        }
    }

    pub(crate) fn workspace_has_fullscreen(&self, index: usize) -> bool {
        let workspace = &self.workspaces[index];
        workspace.fullscreen.is_some_and(|id| {
            workspace.windows.iter().any(|entry| {
                entry.id == id && !entry.minimized && !workspace.hidden_by_parent(entry)
            })
        })
    }

    fn workspace_layout(&self, index: usize) -> Vec<WindowPlacement> {
        let workspace = &self.workspaces[index];
        let fullscreen = workspace.fullscreen.filter(|id| {
            workspace.windows.iter().any(|entry| {
                entry.id == *id && !entry.minimized && !workspace.hidden_by_parent(entry)
            })
        });
        let count = workspace
            .windows
            .iter()
            .filter(|entry| !entry.minimized && entry.floating.is_none())
            .count();
        let area = self.usable_area();
        let mut tiles = master_stack_layout(area.size, count).into_iter();
        let mut result = Vec::new();
        for entry in &workspace.windows {
            let is_fullscreen = workspace.fullscreen == Some(entry.id);
            let base = if let Some(rect) = entry.floating {
                let (min, max) = Self::window_constraints(&entry.window);
                constrain_geometry(rect, self.output_size, min, max)
            } else if !entry.minimized {
                let (loc, size) = tiles.next().unwrap();
                Rectangle::new(loc + area.loc, size)
            } else {
                Rectangle::from_size(self.output_size)
            };
            let rect = if is_fullscreen {
                Rectangle::from_size(self.output_size)
            } else {
                base
            };
            let visible = !entry.minimized
                && !workspace.hidden_by_parent(entry)
                && fullscreen.is_none_or(|owner| {
                    owner == entry.id
                        || (entry.floating.is_some() && workspace.descendant_of(entry, owner))
                });
            result.push((
                entry.window.clone(),
                rect,
                is_fullscreen,
                entry.floating.is_none(),
                visible,
            ));
        }
        // Map tiles first, then fullscreen, then its floating dialogs.
        result.sort_by_key(|(_, _, fullscreen, tiled, _)| {
            if *fullscreen {
                1
            } else if *tiled {
                0
            } else {
                2
            }
        });
        result
    }

    pub(crate) fn workspace_preview_scene(&self, index: usize) -> Option<WorkspacePreviewScene> {
        (index < self.workspaces.len()).then(|| WorkspacePreviewScene {
            output_size: self.output_size,
            windows: self
                .workspace_layout(index)
                .into_iter()
                .filter_map(|(window, geometry, _, _, visible)| {
                    visible.then_some((window, geometry))
                })
                .collect(),
        })
    }

    fn configure_workspace(&self, index: usize) {
        for (window, rect, fullscreen, tiled, _) in self.workspace_layout(index) {
            Self::configure_window(&window, rect.loc, rect.size, fullscreen, tiled);
        }
    }

    pub fn set_window_fullscreen(&mut self, window: &Window, fullscreen: bool) {
        let Some(index) = self.workspace_for_window(window) else {
            return;
        };
        let Some(entry) = self.workspaces[index]
            .windows
            .iter()
            .find(|entry| entry.window == *window)
        else {
            return;
        };
        let id = entry.id;
        if fullscreen == (self.workspaces[index].fullscreen == Some(id)) {
            // xdg-shell requires a configure response even to a repeated request.
            if let Some(surface) = window.toplevel() {
                surface.send_configure();
            }
            return;
        }
        if fullscreen {
            self.workspaces[index].fullscreen = Some(id);
        } else if self.workspaces[index].fullscreen == Some(id) {
            self.workspaces[index].fullscreen = None;
        }
        if index == self.active_workspace {
            self.release_pointer_buttons();
            self.relayout_active_workspace();
        } else {
            self.configure_workspace(index);
        }
    }

    pub(crate) fn floating_geometry(&self, window: &Window) -> Option<Geometry> {
        let workspace = &self.workspaces[self.workspace_for_window(window)?];
        workspace
            .windows
            .iter()
            .find(|entry| {
                entry.window == *window
                    && !entry.minimized
                    && workspace.fullscreen != Some(entry.id)
            })
            .and_then(|entry| entry.floating)
    }

    /// Configure/map a floating window without re-entering pointer dispatch.
    pub(crate) fn set_floating_geometry(&mut self, window: &Window, geometry: Geometry) {
        if self.floating_geometry(window).is_none() {
            return;
        }
        let (min, max) = Self::window_constraints(window);
        let geometry = constrain_geometry(geometry, self.output_size, min, max);
        let index = self.workspace_for_window(window).unwrap();
        let entry = self.workspaces[index]
            .windows
            .iter_mut()
            .find(|entry| entry.window == *window)
            .unwrap();
        entry.floating = Some(geometry);
        Self::configure_window(window, geometry.loc, geometry.size, false, false);
        if self.space.element_location(window).is_some() {
            self.space.map_element(window.clone(), geometry.loc, false);
            self.sync_x11_stacking();
            self.request_repaint();
        }
    }

    pub fn spawn(&mut self, argv: Vec<String>) -> std::io::Result<()> {
        let Some((program, arguments)) = argv.split_first() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "command is empty",
            ));
        };
        self.reap_children();
        let workspace = self.active_workspace;
        let mut command = Command::new(program);
        command
            .args(arguments)
            .envs(&self.config.environment)
            // A shell started from a desktop terminal can inherit the host's
            // X11 display and a forced GTK backend. Neither describes the
            // session that Villain is providing to this child.
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .env("WAYLAND_DISPLAY", &self.socket_name);
        if let Some(display) = self.xwayland_display {
            command.env("DISPLAY", format!(":{display}"));
        }
        let child = command.spawn()?;
        tracing::info!(
            pid = child.id(),
            workspace = workspace + 1,
            ?argv,
            "application launched"
        );
        self.children.push((workspace, child));
        Ok(())
    }
    pub fn reap_children(&mut self) {
        self.children
            .retain_mut(|(_, child)| match child.try_wait() {
                Ok(Some(status)) => {
                    tracing::info!(pid = child.id(), %status, "launched app exited");
                    false
                }
                Ok(None) => true,
                Err(error) => {
                    tracing::warn!(%error, "could not reap app");
                    true
                }
            });
    }
    fn configure_window(
        window: &Window,
        location: Point<i32, smithay::utils::Logical>,
        size: Size<i32, smithay::utils::Logical>,
        fullscreen: bool,
        tiled: bool,
    ) {
        if let Some(surface) = window.toplevel() {
            surface.with_pending_state(|pending| {
                pending.size = Some(size);
                for state in [
                    xdg_toplevel::State::TiledLeft,
                    xdg_toplevel::State::TiledRight,
                    xdg_toplevel::State::TiledTop,
                    xdg_toplevel::State::TiledBottom,
                ] {
                    if tiled && !fullscreen {
                        pending.states.set(state);
                    } else {
                        pending.states.unset(state);
                    }
                }
                if fullscreen {
                    pending.states.set(xdg_toplevel::State::Fullscreen);
                } else {
                    pending.states.unset(xdg_toplevel::State::Fullscreen);
                    pending.fullscreen_output = None;
                }
            });
            surface.send_pending_configure();
        } else if let Some(surface) = window.x11_surface() {
            if surface.is_fullscreen() != fullscreen
                && let Err(error) = surface.set_fullscreen(fullscreen)
            {
                tracing::warn!(%error, "could not update X11 fullscreen state");
            }
            let geometry = Rectangle::new(location, size);
            if surface.geometry() != geometry
                && let Err(error) = surface.configure(geometry)
            {
                tracing::warn!(%error, window = surface.window_id(), "could not configure X11 window");
            }
        }
    }

    fn send_pending_configure(window: &Window) {
        if let Some(surface) = window.toplevel() {
            surface.send_pending_configure();
        }
    }

    /// Update a window's activation state and notify the client only when the
    /// state actually changed. Sending an activation configure for every
    /// window during every relayout creates a burst of competing configure
    /// serials while the workspace is being switched.
    pub(crate) fn set_activated(window: &Window, activated: bool) {
        if window.set_activated(activated) {
            Self::send_pending_configure(window);
        }
    }

    fn window_surface(
        window: &Window,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        window.wl_surface().map(|surface| surface.into_owned())
    }

    fn workspace_for_pid(&self, pid: Option<u32>) -> usize {
        self.children
            .iter()
            .find(|(_, child)| Some(child.id()) == pid)
            .map(|(index, _)| *index)
            .unwrap_or(self.active_workspace)
    }

    fn add_workspace_window(&mut self, index: usize, window: Window) {
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;
        self.workspaces[index].windows.push(WorkspaceWindow {
            id,
            window: window.clone(),
            minimized: false,
            parent: None,
            floating: None,
        });
        self.refresh_window_hints(&window);
        tracing::info!(window = id.0, workspace = index + 1, "window opened");
        let index = self.workspace_for_window(&window).unwrap_or(index);
        self.remember_focused_window(&window);
        if index == self.active_workspace {
            self.relayout_active_workspace();
            self.focus_managed_window(&window);
        }
    }

    pub fn add_window(&mut self, surface: ToplevelSurface) {
        // Associate a direct child with its launch workspace even after switching.
        let pid = surface
            .wl_surface()
            .client()
            .and_then(|client| client.get_credentials(&self.display_handle).ok())
            .map(|credentials| credentials.pid as u32);
        let index = self.workspace_for_pid(pid);
        self.add_workspace_window(index, Window::new_wayland_window(surface));
    }

    pub fn add_x11_window(&mut self, surface: X11Surface) {
        if self.window_for_x11_surface(&surface).is_some() {
            return;
        }
        let index = self.workspace_for_pid(surface.get_client_pid().ok());
        self.add_workspace_window(index, Window::new_x11_window(surface));
    }
    pub fn switch_workspace(&mut self, index: usize) {
        if index >= self.workspaces.len() {
            return;
        }
        self.remember_current_window_focus();
        if !self
            .pointer
            .grab_start_data()
            .and_then(|start| start.focus)
            .is_some_and(|(surface, _)| self.layer_surface_visible(&surface))
        {
            self.release_pointer_buttons();
        }
        let old: Vec<_> = self
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.windows.iter().map(|entry| entry.window.clone()))
            .collect();
        for window in old {
            self.space.unmap_elem(&window);
            Self::set_activated(&window, false);
        }
        self.active_workspace = index;
        self.relayout_active_workspace();
    }

    pub fn relayout_active_workspace(&mut self) {
        self.arrange_layers();
        let old: Vec<_> = self
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.windows.iter().map(|entry| entry.window.clone()))
            .collect();
        for window in old {
            self.space.unmap_elem(&window);
        }

        for (window, geometry, fullscreen, tiled, visible) in
            self.workspace_layout(self.active_workspace)
        {
            Self::configure_window(&window, geometry.loc, geometry.size, fullscreen, tiled);
            if visible {
                self.space.map_element(window, geometry.loc, false);
            } else {
                Self::set_activated(&window, false);
            }
        }
        self.refresh_unmanaged_x11_windows();
        self.sync_x11_stacking();
        // An implicit button grab must not survive minimizing its window.
        if self
            .pointer
            .current_focus()
            .or_else(|| {
                self.pointer
                    .grab_start_data()
                    .and_then(|start| start.focus.map(|(surface, _)| surface))
            })
            .is_some_and(|surface| {
                let root =
                    std::iter::successors(Some(surface), smithay::wayland::compositor::get_parent)
                        .last()
                        .unwrap();
                !self.layer_surface_visible(&root)
                    && !self
                        .window_for_surface(&root)
                        .is_some_and(|window| self.space.element_location(&window).is_some())
            })
        {
            self.release_pointer_buttons();
        }
        // Workspace transitions must not let the stale pointer location choose
        // a different application before the workspace focus policy runs.
        self.refresh_pointer_surface(0);
        self.restore_active_workspace_focus();
    }

    pub fn close_focused_window(&mut self) -> bool {
        if let Some(window) = self.focused_window() {
            self.apply_window_action(&window, WindowAction::Close);
            true
        } else {
            false
        }
    }

    pub fn minimize_focused_window(&mut self) -> bool {
        if let Some(window) = self.focused_window() {
            self.apply_window_action(&window, WindowAction::Minimize);
            true
        } else {
            false
        }
    }

    pub(crate) fn apply_window_action(&mut self, window: &Window, action: WindowAction) {
        match action {
            WindowAction::Close => {
                if let Some(surface) = window.toplevel() {
                    surface.send_close();
                } else if let Some(surface) = window.x11_surface()
                    && let Err(error) = surface.close()
                {
                    tracing::warn!(%error, window = surface.window_id(), "could not close X11 window");
                }
            }
            WindowAction::Minimize => {
                let mut changed_active_workspace = false;
                for (index, workspace) in self.workspaces.iter_mut().enumerate() {
                    let Some(entry) = workspace
                        .windows
                        .iter_mut()
                        .find(|entry| entry.window == *window && !entry.minimized)
                    else {
                        continue;
                    };
                    entry.minimized = true;
                    Self::set_activated(&entry.window, false);
                    let window = entry.window.clone();
                    workspace
                        .minimized_history
                        .retain(|candidate| candidate != &window);
                    workspace.minimized_history.push(window);
                    changed_active_workspace = index == self.active_workspace;
                    break;
                }
                if changed_active_workspace {
                    self.relayout_active_workspace();
                }
            }
        }
    }

    pub fn restore_last_minimized_window(&mut self) -> bool {
        let workspace = &mut self.workspaces[self.active_workspace];
        while let Some(window) = workspace.minimized_history.pop() {
            if let Some(entry) = workspace
                .windows
                .iter_mut()
                .find(|entry| entry.window == window && entry.minimized)
            {
                entry.minimized = false;
                self.relayout_active_workspace();
                return true;
            }
        }
        false
    }

    /// Returns `None` for an unknown ID and `Some(false)` for a minimized one.
    pub fn focus_window(&mut self, id: WindowId) -> Option<bool> {
        let (workspace, minimized, surface) =
            self.workspaces
                .iter()
                .enumerate()
                .find_map(|(workspace, state)| {
                    state
                        .windows
                        .iter()
                        .find(|entry| entry.id == id)
                        .map(|entry| {
                            (
                                workspace,
                                entry.minimized || state.hidden_by_parent(entry),
                                KeyboardFocus::for_window(&entry.window),
                            )
                        })
                })?;
        if minimized {
            return Some(false);
        }
        if workspace != self.active_workspace {
            self.switch_workspace(workspace);
        }
        if !self
            .workspace_layout(workspace)
            .iter()
            .any(|(window, _, _, _, visible)| {
                *visible && KeyboardFocus::for_window(window) == surface
            })
        {
            self.workspaces[workspace].fullscreen = None;
            self.relayout_active_workspace();
        }
        let Some(surface) = surface else {
            return Some(false);
        };
        let window = self.workspaces[self.active_workspace]
            .windows
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.window.clone());
        if let Some(window) = window {
            debug_assert_eq!(KeyboardFocus::for_window(&window), Some(surface));
            self.focus_managed_window(&window);
        }
        Some(true)
    }

    pub fn restore_window(&mut self, id: WindowId) -> bool {
        for (workspace_index, workspace) in self.workspaces.iter_mut().enumerate() {
            let Some(entry) = workspace.windows.iter_mut().find(|entry| entry.id == id) else {
                continue;
            };
            if entry.minimized {
                entry.minimized = false;
                let window = entry.window.clone();
                workspace
                    .minimized_history
                    .retain(|candidate| candidate != &window);
                if workspace_index == self.active_workspace {
                    self.relayout_active_workspace();
                }
            }
            return true;
        }
        false
    }

    pub fn window_info(&self) -> Vec<WindowInfo> {
        let focused = self
            .keyboard
            .current_focus()
            .and_then(|focus| focus.wl_surface().map(|surface| surface.into_owned()));
        self.workspaces
            .iter()
            .enumerate()
            .flat_map(|(workspace, state)| {
                let focused = focused.clone();
                state.windows.iter().map(move |entry| {
                    let (title, app_id, wl_surface) = if let Some(surface) = entry.window.toplevel()
                    {
                        let (title, app_id) = smithay::wayland::compositor::with_states(
                            surface.wl_surface(),
                            |states| {
                                let attributes = states
                                    .data_map
                                    .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                                    .expect("XDG toplevel data")
                                    .lock()
                                    .unwrap();
                                (
                                    attributes.title.clone().unwrap_or_default(),
                                    attributes.app_id.clone().unwrap_or_default(),
                                )
                            },
                        );
                        (title, app_id, Some(surface.wl_surface().clone()))
                    } else if let Some(surface) = entry.window.x11_surface() {
                        (surface.title(), surface.class(), surface.wl_surface())
                    } else {
                        (String::new(), String::new(), None)
                    };
                    WindowInfo {
                        id: entry.id,
                        title,
                        app_id,
                        workspace: WorkspaceId((workspace + 1) as u32),
                        minimized: entry.minimized,
                        floating: entry.floating.is_some(),
                        fullscreen: state.fullscreen == Some(entry.id),
                        focused: focused
                            .as_ref()
                            .is_some_and(|focused| wl_surface.as_ref() == Some(focused)),
                    }
                })
            })
            .collect()
    }

    pub fn workspace_info(&self) -> Vec<WorkspaceInfo> {
        self.workspaces
            .iter()
            .enumerate()
            .map(|(index, workspace)| WorkspaceInfo {
                workspace: WorkspaceId((index + 1) as u32),
                active: index == self.active_workspace,
                window_count: workspace.windows.len() as u32,
                visible_window_count: self
                    .workspace_layout(index)
                    .iter()
                    .filter(|(_, _, _, _, visible)| *visible)
                    .count() as u32,
            })
            .collect()
    }

    pub fn active_window_info(&self) -> Option<WindowInfo> {
        self.window_info().into_iter().find(|window| window.focused)
    }

    fn focused_window(&self) -> Option<Window> {
        let focused = self.keyboard.current_focus()?;
        self.workspaces[self.active_workspace]
            .windows
            .iter()
            .find(|entry| KeyboardFocus::for_window(&entry.window).as_ref() == Some(&focused))
            .map(|entry| entry.window.clone())
    }

    fn remember_focused_window(&mut self, window: &Window) {
        for workspace in &mut self.workspaces {
            let Some(id) = workspace
                .windows
                .iter()
                .find(|entry| entry.window == *window)
                .map(|entry| entry.id)
            else {
                continue;
            };
            workspace.focus_history.retain(|candidate| *candidate != id);
            workspace.focus_history.push(id);
            return;
        }
    }

    fn remember_current_window_focus(&mut self) {
        let Some(focus) = self.keyboard.current_focus() else {
            return;
        };
        let window = self
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.windows.iter())
            .find(|entry| KeyboardFocus::for_window(&entry.window).as_ref() == Some(&focus))
            .map(|entry| entry.window.clone());
        if let Some(window) = window {
            self.remember_focused_window(&window);
        }
    }

    fn window_id(&self, window: &Window) -> Option<WindowId> {
        self.workspaces
            .iter()
            .flat_map(|workspace| workspace.windows.iter())
            .find(|entry| entry.window == *window)
            .map(|entry| entry.id)
    }

    fn window_is_visible(&self, index: usize, id: WindowId) -> bool {
        let Some(entry) = self.workspaces[index]
            .windows
            .iter()
            .find(|entry| entry.id == id)
        else {
            return false;
        };
        self.workspace_layout(index)
            .into_iter()
            .any(|(window, _, _, _, visible)| visible && window == entry.window)
    }

    fn last_visible_window(&self, index: usize) -> Option<Window> {
        let history = self.workspaces[index].focus_history.iter().rev().copied();
        for id in history {
            if self.window_is_visible(index, id) {
                return self.workspaces[index]
                    .windows
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| entry.window.clone());
            }
        }
        self.workspaces[index]
            .windows
            .iter()
            .rev()
            .find(|entry| self.window_is_visible(index, entry.id))
            .map(|entry| entry.window.clone())
    }

    fn active_focus_is_valid(&self) -> bool {
        let Some(focus) = self.keyboard.current_focus() else {
            return false;
        };
        if self
            .exclusive_layer_focus()
            .as_ref()
            .is_some_and(|layer| layer == &focus)
        {
            return true;
        }
        self.workspaces[self.active_workspace]
            .windows
            .iter()
            .any(|entry| {
                self.window_is_visible(self.active_workspace, entry.id)
                    && KeyboardFocus::for_window(&entry.window).as_ref() == Some(&focus)
            })
    }

    fn focus_managed_window(&mut self, window: &Window) -> bool {
        let Some(index) = self.workspace_for_window(window) else {
            return false;
        };
        let Some(id) = self.window_id(window) else {
            return false;
        };
        if index != self.active_workspace || !self.window_is_visible(index, id) {
            return false;
        }
        let Some(surface) = KeyboardFocus::for_window(window) else {
            return false;
        };
        self.remember_focused_window(window);
        for entry in &self.workspaces[self.active_workspace].windows {
            Self::set_activated(&entry.window, entry.window == *window);
        }
        let surface = self.exclusive_layer_focus().unwrap_or(surface);
        if self.keyboard.current_focus().as_ref() != Some(&surface) {
            // Compositor shortcuts are intercepted, so Smithay's forwarded
            // key set does not contain the keys that are still held.  Moving
            // keyboard focus now would send the new client a modifier state
            // without the matching key press/release lifecycle.  Wait until
            // the shortcut is released and the keyboard state is neutral.
            if !self.suppressed_keys.is_empty() {
                self.pending_focus_restore = true;
                return true;
            }
            self.keyboard
                .clone()
                .set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
            self.pending_focus_restore = false;
        }
        true
    }

    pub fn restore_active_workspace_focus(&mut self) {
        self.restore_active_workspace_focus_inner(false);
    }

    fn restore_active_workspace_focus_inner(&mut self, force: bool) {
        if !self.host_focused
            || self.keyboard.is_grabbed()
            || (!force && self.active_focus_is_valid())
        {
            return;
        }
        if !self.suppressed_keys.is_empty() {
            self.pending_focus_restore = true;
            return;
        }
        if let Some(window) = self.last_visible_window(self.active_workspace) {
            self.focus_managed_window(&window);
        } else if self.keyboard.current_focus().is_some() {
            self.keyboard
                .clone()
                .set_focus(self, None, SERIAL_COUNTER.next_serial());
            self.pending_focus_restore = false;
        }
    }

    /// Apply a focus transition postponed while a compositor shortcut was held.
    pub fn flush_pending_focus(&mut self) {
        if self.pending_focus_restore
            && self.suppressed_keys.is_empty()
            && self.host_focused
            && !self.keyboard.is_grabbed()
        {
            self.pending_focus_restore = false;
            self.restore_active_workspace_focus_inner(true);
        }
    }

    fn window_for_toplevel(&self, surface: &ToplevelSurface) -> Option<Window> {
        self.workspaces
            .iter()
            .flat_map(|workspace| &workspace.windows)
            .find(|entry| entry.window.toplevel() == Some(surface))
            .map(|entry| entry.window.clone())
    }

    pub fn window_for_surface(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) -> Option<Window> {
        self.workspaces
            .iter()
            .flat_map(|workspace| &workspace.windows)
            .find(|entry| Self::window_surface(&entry.window).as_ref() == Some(surface))
            .map(|entry| entry.window.clone())
            .or_else(|| {
                self.unmanaged_x11_windows
                    .iter()
                    .find(|entry| Self::window_surface(&entry.window).as_ref() == Some(surface))
                    .map(|entry| entry.window.clone())
            })
    }

    pub fn workspace_for_window(&self, window: &Window) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|workspace| {
                workspace
                    .windows
                    .iter()
                    .any(|entry| entry.window == *window)
            })
            .or_else(|| {
                self.unmanaged_x11_windows
                    .iter()
                    .find(|entry| entry.window == *window)
                    .map(|entry| entry.workspace)
            })
    }

    pub fn window_for_x11_surface(&self, surface: &X11Surface) -> Option<Window> {
        self.workspaces
            .iter()
            .flat_map(|workspace| &workspace.windows)
            .map(|entry| &entry.window)
            .chain(self.unmanaged_x11_windows.iter().map(|entry| &entry.window))
            .find(|window| window.x11_surface() == Some(surface))
            .cloned()
    }

    pub fn window_for_x11_id(&self, id: u32) -> Option<Window> {
        self.workspaces
            .iter()
            .flat_map(|workspace| &workspace.windows)
            .map(|entry| &entry.window)
            .chain(self.unmanaged_x11_windows.iter().map(|entry| &entry.window))
            .find(|window| {
                window
                    .x11_surface()
                    .is_some_and(|surface| surface.window_id() == id)
            })
            .cloned()
    }

    fn focus_layer_at_pointer(&mut self) -> bool {
        if let Some(focus) = self.exclusive_layer_focus() {
            self.focus_layer(focus);
            return true;
        }
        if self.host_focused
            && let Some((_, _, Some(focus))) = self.layer_under_pointer(true)
        {
            self.focus_layer(focus);
            return true;
        }
        if self.host_focused
            && self.space.element_under(self.pointer_location).is_none()
            && let Some((_, _, Some(focus))) = self.layer_under_pointer(false)
        {
            self.focus_layer(focus);
            return true;
        }
        false
    }

    pub fn focus_window_at_pointer(&mut self) {
        if self.focus_layer_at_pointer() {
            return;
        }
        let hit = self
            .space
            .element_under(self.pointer_location)
            .map(|(window, location)| (window.clone(), location));
        let keyboard_surface =
            hit.as_ref()
                .filter(|_| self.host_focused)
                .and_then(|(window, _)| {
                    if window
                        .x11_surface()
                        .is_some_and(|surface| surface.is_override_redirect())
                    {
                        self.unmanaged_x11_windows
                            .iter()
                            .find(|entry| entry.window == *window)
                            .and_then(|entry| entry.parent.as_ref())
                            .and_then(KeyboardFocus::for_window)
                            .or_else(|| {
                                self.keyboard.current_focus().filter(|focus| {
                                    self.space.elements().any(|window| {
                                        KeyboardFocus::for_window(window).as_ref() == Some(focus)
                                    })
                                })
                            })
                    } else {
                        KeyboardFocus::for_window(window)
                    }
                });
        let focused_window = keyboard_surface.as_ref().and_then(|focus| {
            self.workspaces
                .iter()
                .flat_map(|workspace| workspace.windows.iter())
                .find(|entry| KeyboardFocus::for_window(&entry.window).as_ref() == Some(focus))
                .map(|entry| entry.window.clone())
        });
        if let Some(window) = focused_window.as_ref() {
            self.remember_focused_window(window);
        }
        if self.keyboard.current_focus() != keyboard_surface {
            for entry in &self.workspaces[self.active_workspace].windows {
                let activated = keyboard_surface.as_ref().is_some_and(|surface| {
                    KeyboardFocus::for_window(&entry.window).as_ref() == Some(surface)
                });
                Self::set_activated(&entry.window, activated);
            }
            let keyboard = self.keyboard.clone();
            keyboard.set_focus(self, keyboard_surface, SERIAL_COUNTER.next_serial());
        }
    }
    /// End any drag before focus moves away from its application.
    pub fn release_pointer_buttons(&mut self) {
        let pointer = self.pointer.clone();
        for button in std::mem::take(&mut self.pressed_buttons) {
            pointer.button(
                self,
                &smithay::input::pointer::ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: 0,
                    button,
                    state: smithay::backend::input::ButtonState::Released,
                },
            );
        }
        pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), 0);
        pointer.frame(self);
    }
    pub fn refresh_pointer(&mut self, time: u32) {
        self.request_repaint();
        self.refresh_pointer_surface(time);
        if !self.host_focused {
            if self.keyboard.current_focus().is_some() {
                self.keyboard
                    .clone()
                    .set_focus(self, None, SERIAL_COUNTER.next_serial());
            }
        } else if !self.pointer.is_grabbed() {
            self.focus_layer_at_pointer();
        }
    }

    pub fn refresh_pointer_and_focus(&mut self, time: u32) {
        self.request_repaint();
        self.refresh_pointer_surface(time);
        if !self.pointer.is_grabbed() {
            self.focus_window_at_pointer();
        }
    }

    pub fn refresh_pointer_surface(&mut self, time: u32) {
        let hit = self
            .space
            .element_under(self.pointer_location)
            .map(|(window, location)| (window.clone(), location));
        let focus = if self.host_focused {
            self.layer_under_pointer(true)
                .map(|(surface, origin, _)| (surface, origin))
                .or_else(|| {
                    hit.and_then(|(window, origin)| {
                        window
                            .surface_under(
                                self.pointer_location - origin.to_f64(),
                                WindowSurfaceType::ALL,
                            )
                            .map(|(surface, location)| {
                                (surface, location.to_f64() + origin.to_f64())
                            })
                    })
                })
                .or_else(|| {
                    self.layer_under_pointer(false)
                        .map(|(surface, origin, _)| (surface, origin))
                })
        } else {
            None
        };
        let pointer = self.pointer.clone();
        pointer.motion(
            self,
            focus,
            &MotionEvent {
                location: self.pointer_location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    pub fn remove_window(&mut self, surface: &ToplevelSurface) {
        if let Some(window) = self.window_for_toplevel(surface) {
            self.remove_managed_window(&window);
        }
    }

    pub fn remove_x11_window(&mut self, surface: &X11Surface) {
        if let Some(window) = self.window_for_x11_surface(surface) {
            if surface.is_override_redirect() {
                if self.pointer.current_focus() == Self::window_surface(&window) {
                    self.release_pointer_buttons();
                }
                self.space.unmap_elem(&window);
                self.unmanaged_x11_windows
                    .retain(|entry| entry.window != window);
                self.refresh_unmanaged_x11_windows();
                self.sync_x11_stacking();
                self.refresh_pointer(0);
            } else {
                self.remove_managed_window(&window);
            }
        }
    }

    fn remove_managed_window(&mut self, target: &Window) {
        for workspace in &mut self.workspaces {
            let removed: Vec<_> = workspace
                .windows
                .extract_if(.., |entry| &entry.window == target)
                .map(|entry| (entry.id, entry.window))
                .collect();
            if !workspace
                .windows
                .iter()
                .any(|entry| Some(entry.id) == workspace.fullscreen)
            {
                workspace.fullscreen = None;
            }
            for (id, window) in removed {
                self.space.unmap_elem(&window);
                workspace.focus_history.retain(|candidate| *candidate != id);
                workspace
                    .minimized_history
                    .retain(|candidate| candidate != &window);
            }
        }
        self.relayout_active_workspace();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_stack_layouts_zero_to_three_windows() {
        let output = (800, 600).into();
        assert!(master_stack_layout(output, 0).is_empty());
        assert_eq!(
            master_stack_layout(output, 1),
            vec![((0, 0).into(), (800, 600).into())]
        );
        assert_eq!(
            master_stack_layout(output, 2),
            vec![
                ((0, 0).into(), (400, 600).into()),
                ((400, 0).into(), (400, 600).into()),
            ]
        );
        assert_eq!(
            master_stack_layout(output, 3),
            vec![
                ((0, 0).into(), (400, 600).into()),
                ((400, 0).into(), (400, 300).into()),
                ((400, 300).into(), (400, 300).into()),
            ]
        );
    }

    #[test]
    fn stack_absorbs_integer_remainders() {
        let layout = master_stack_layout((801, 601).into(), 4);
        assert_eq!(layout[0], ((0, 0).into(), (400, 601).into()));
        assert_eq!(layout[3], ((400, 400).into(), (401, 201).into()));
    }
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod request_tests;
