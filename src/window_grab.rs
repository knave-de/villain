//! Pointer-owned window placement. Client requests retain protocol authentication.
use crate::{dispatch::DispatchError, state::Villain, workspaces::SplitContext};
use knave_desktop_api::{ResizeEdge as PublicEdge, WindowId, WindowTarget};
use smithay::{
    backend::input::ButtonState,
    desktop::Window,
    input::pointer::*,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial, Size},
    wayland::seat::WaylandFocus,
    xwayland::xwm::ResizeEdge,
};
type Geometry = Rectangle<i32, Logical>;
type PointerFocus = Option<(WlSurface, Point<f64, Logical>)>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeEdges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}
impl ResizeEdges {
    pub fn from_xdg(edge: xdg_toplevel::ResizeEdge) -> Option<Self> {
        use xdg_toplevel::ResizeEdge::*;
        Some(Self::from_x11(match edge {
            Top => ResizeEdge::Top,
            Bottom => ResizeEdge::Bottom,
            Left => ResizeEdge::Left,
            Right => ResizeEdge::Right,
            TopLeft => ResizeEdge::TopLeft,
            TopRight => ResizeEdge::TopRight,
            BottomLeft => ResizeEdge::BottomLeft,
            BottomRight => ResizeEdge::BottomRight,
            _ => return Option::None,
        }))
    }
    pub fn from_x11(edge: ResizeEdge) -> Self {
        use ResizeEdge::*;
        Self {
            left: matches!(edge, Left | TopLeft | BottomLeft),
            right: matches!(edge, Right | TopRight | BottomRight),
            top: matches!(edge, Top | TopLeft | TopRight),
            bottom: matches!(edge, Bottom | BottomLeft | BottomRight),
        }
    }
    fn from_public(edge: PublicEdge) -> Self {
        Self::from_x11(match edge {
            PublicEdge::Top => ResizeEdge::Top,
            PublicEdge::Bottom => ResizeEdge::Bottom,
            PublicEdge::Left => ResizeEdge::Left,
            PublicEdge::Right => ResizeEdge::Right,
            PublicEdge::TopLeft => ResizeEdge::TopLeft,
            PublicEdge::TopRight => ResizeEdge::TopRight,
            PublicEdge::BottomLeft => ResizeEdge::BottomLeft,
            PublicEdge::BottomRight => ResizeEdge::BottomRight,
        })
    }
    fn nearest(rect: Geometry, point: Point<f64, Logical>) -> Self {
        let left = point.x < f64::from(rect.loc.x) + f64::from(rect.size.w) / 2.0;
        let top = point.y < f64::from(rect.loc.y) + f64::from(rect.size.h) / 2.0;
        Self {
            left,
            right: !left,
            top,
            bottom: !top,
        }
    }
    fn cursor(self) -> CursorIcon {
        match (self.left || self.right, self.top || self.bottom) {
            (true, true) if self.left == self.top => CursorIcon::NwseResize,
            (true, true) => CursorIcon::NeswResize,
            (true, false) => CursorIcon::EwResize,
            _ => CursorIcon::NsResize,
        }
    }
}

pub(crate) enum Operation {
    Floating {
        window: Window,
        geometry: Geometry,
        edges: Option<ResizeEdges>,
    },
    Tile {
        window: Window,
        geometry: Geometry,
        destination: Option<Window>,
    },
    Master {
        context: SplitContext,
        initial: Option<u16>,
        offset: f64,
    },
    Stack {
        context: SplitContext,
        weights: Vec<(WindowId, u32)>,
        pair: usize,
        upper: i32,
        lower: i32,
    },
}
pub(crate) struct Interaction {
    pub(crate) operation: Operation,
    start: Point<f64, Logical>,
    button: Option<u32>,
    workspace: usize,
    area: Geometry,
    output: Size<i32, Logical>,
    pub(crate) delta: Point<i32, Logical>,
    pub(crate) moved: bool,
}

fn window_menu_row(
    rect: smithay::utils::Rectangle<i32, Logical>,
    location: Point<f64, Logical>,
) -> Option<u8> {
    if !rect.to_f64().contains(location) {
        return None;
    }
    let row = ((location.y - f64::from(rect.loc.y)) / 30.0) as u8;
    (row < 4).then_some(row)
}

impl Villain {
    /// Shared Winit/libinput button path, before any client delivery.
    pub(crate) fn handle_compositor_button(
        &mut self,
        button: u32,
        state: ButtonState,
        time: u32,
    ) -> bool {
        self.finish_workspace_motion();

        if self.window_menu.is_some() {
            if state == ButtonState::Released {
                let swallowed = self.swallowed_buttons.remove(&button);
                self.pressed_buttons.remove(&button);
                return swallowed;
            }
            if state == ButtonState::Pressed {
                self.pressed_buttons.insert(button);
                self.swallowed_buttons.insert(button);
                let inside = self
                    .window_menu
                    .as_ref()
                    .is_some_and(|menu| menu.rect.to_f64().contains(self.pointer_location));
                let row = self.window_menu.as_ref().and_then(|menu| {
                    (button == 0x110 && inside)
                        .then(|| window_menu_row(menu.rect, self.pointer_location))
                        .flatten()
                });
                if let Some(row) = row {
                    self.activate_window_menu_item(row);
                } else if !inside {
                    self.window_menu = None;
                    self.request_repaint();
                }
                return true;
            }
        }

        if state == ButtonState::Pressed {
            self.swallowed_buttons.remove(&button);
            self.pressed_buttons.insert(button);
            if self
                .interaction
                .as_ref()
                .is_some_and(|drag| drag.button.unwrap_or(0x110) != button)
            {
                self.swallowed_buttons.insert(button);
                return true;
            }
            let mods = self.keyboard.modifier_state();
            if mods.logo || mods.alt {
                self.pending_modifier = None;
            }
            if button == 0x111 && mods.logo && self.open_window_menu_at_pointer() {
                self.swallowed_buttons.insert(button);
                return true;
            }
            if self.can_start_interaction()
                && self.layer_under_pointer(true).is_none()
                && let Some(dispatch) = self.config.keybinds.find_mouse(button, mods)
            {
                self.swallowed_buttons.insert(button);
                if let Err(error) = self.dispatch(dispatch) {
                    tracing::debug!(%error, "mouse binding had no effect");
                }
                return true;
            }
        } else {
            let swallowed = self.swallowed_buttons.remove(&button);
            if !self.pressed_buttons.remove(&button) {
                return true;
            }
            if swallowed
                && self
                    .interaction
                    .as_ref()
                    .is_none_or(|drag| drag.button.unwrap_or(0x110) != button)
            {
                return true;
            }
        }
        if button == 0x111 && state == ButtonState::Pressed && self.open_window_menu_from_titlebar()
        {
            self.swallowed_buttons.insert(button);
            return true;
        }
        if button == 0x110
            && state == ButtonState::Pressed
            && self.handle_gallery_titlebar(button, time)
        {
            return true;
        }
        self.try_start_edge_resize(button, state) || self.try_start_split_drag(button, state, time)
    }

    fn open_window_menu_from_titlebar(&mut self) -> bool {
        if !self.can_open_window_menu(false) || self.layer_under_pointer(true).is_some() {
            return false;
        }
        let hit = self
            .workspace_layout(self.active_workspace)
            .into_iter()
            .rev()
            .find_map(|(window, rect, fullscreen, maximized, tiled, visible)| {
                if !visible {
                    return None;
                }
                let bar = crate::workspaces::titlebar_rect(&window, rect, fullscreen)?;
                bar.to_f64()
                    .contains(self.pointer_location)
                    .then_some((window, bar, maximized, tiled))
            });
        let Some((window, bar, maximized, tiled)) = hit else {
            return false;
        };
        self.open_window_menu(window, self.pointer_location, maximized, tiled, bar);
        true
    }

    fn open_window_menu_at_pointer(&mut self) -> bool {
        if !self.can_open_window_menu(false) || self.layer_under_pointer(true).is_some() {
            return false;
        }
        let Some((window, origin)) = self.input_window_at_pointer() else {
            return false;
        };
        let Some((surface, _)) = window.surface_under(
            self.pointer_location - origin.to_f64(),
            smithay::desktop::WindowSurfaceType::ALL,
        ) else {
            return false;
        };
        if std::iter::successors(Some(surface), smithay::wayland::compositor::get_parent)
            .any(|surface| self.popups.find_popup(&surface).is_some())
        {
            return false;
        }
        let Some((_, rect, fullscreen, maximized, tiled, visible)) = self
            .workspace_layout(self.active_workspace)
            .into_iter()
            .find(|entry| entry.0 == window)
        else {
            return false;
        };
        if !visible || fullscreen {
            return false;
        }
        self.open_window_menu(window, self.pointer_location, maximized, tiled, rect);
        true
    }

    pub(crate) fn open_window_menu(
        &mut self,
        window: Window,
        anchor: smithay::utils::Point<f64, smithay::utils::Logical>,
        maximized: bool,
        tiled: bool,
        _bounds: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    ) {
        let Some(window_id) = self.window_id(&window) else {
            return;
        };
        self.focus_managed_window(&window);
        let width = self.output_size.w.clamp(1, 200);
        let height = self.output_size.h.clamp(1, 120);
        let x = (anchor.x.floor() as i32).clamp(0, (self.output_size.w - width).max(0));
        let y = (anchor.y.floor() as i32).clamp(0, (self.output_size.h - height).max(0));
        let rect = smithay::utils::Rectangle::new((x, y).into(), (width, height).into());
        self.window_menu = Some(crate::state::WindowMenuState {
            window_id,
            rect,
            hovered: None,
            maximized,
            tiled,
        });
        self.request_repaint();
    }

    pub(crate) fn can_open_window_menu(&self, allow_pointer_grab: bool) -> bool {
        self.host_focused
            && self.interaction.is_none()
            && !self.keyboard.is_grabbed()
            && (allow_pointer_grab || !self.pointer.is_grabbed())
            && self.exclusive_layer_focus().is_none()
            && !self.overview_visible
            && !self.overview_mapped()
            && !self.screenshot.intercepting_pointer()
    }

    pub(crate) fn activate_window_menu_item(&mut self, row: u8) {
        let Some(menu) = self.window_menu.take() else {
            return;
        };
        let Some(window) = self.capture_window(menu.window_id) else {
            self.request_repaint();
            return;
        };
        match row {
            0 => self.apply_window_action(&window, crate::workspaces::WindowAction::Minimize),
            1 => {
                if self.focus_managed_window(&window) {
                    let _ = self.toggle_floating_focused();
                }
            }
            2 => self.set_window_maximized(&window, !menu.maximized),
            3 => self.apply_window_action(&window, crate::workspaces::WindowAction::Close),
            _ => {}
        }
        self.request_repaint();
    }

    pub(crate) fn handle_window_menu_key(&mut self, key: u32, pressed: bool) -> bool {
        let Some(menu) = self.window_menu.as_mut() else {
            return false;
        };
        if !pressed {
            return true;
        }
        match key {
            x if x == smithay::input::keyboard::keysyms::KEY_Escape => {
                self.window_menu = None;
            }
            x if x == smithay::input::keyboard::keysyms::KEY_Up => {
                let row = menu.hovered.unwrap_or(0).saturating_sub(1);
                menu.hovered = Some(row);
            }
            x if x == smithay::input::keyboard::keysyms::KEY_Down => {
                let row = menu.hovered.map_or(0, |row| (row + 1).min(3));
                menu.hovered = Some(row);
            }
            x if x == smithay::input::keyboard::keysyms::KEY_Return
                || x == smithay::input::keyboard::keysyms::KEY_KP_Enter =>
            {
                let row = menu.hovered.unwrap_or(0);
                self.activate_window_menu_item(row);
                return true;
            }
            _ => {}
        }
        self.request_repaint();
        true
    }

    pub(crate) fn update_window_menu_hover(&mut self) {
        if let Some(menu) = self.window_menu.as_mut() {
            let hovered = menu.rect;
            let hovered = window_menu_row(hovered, self.pointer_location);
            if menu.hovered != hovered {
                menu.hovered = hovered;
                self.request_repaint();
            }
        }
    }

    fn handle_gallery_titlebar(&mut self, button: u32, time: u32) -> bool {
        if !self.can_start_interaction() || self.layer_under_pointer(true).is_some() {
            return false;
        }
        let hit = self
            .workspace_layout(self.active_workspace)
            .into_iter()
            .rev()
            .find_map(|(window, rect, fullscreen, maximized, _, visible)| {
                if !visible {
                    return None;
                }
                let bar = crate::workspaces::titlebar_rect(&window, rect, fullscreen)?;
                bar.to_f64()
                    .contains(self.pointer_location)
                    .then_some((window, bar, maximized))
            });
        let Some((window, bar, maximized)) = hit else {
            return false;
        };
        self.focus_managed_window(&window);
        self.swallowed_buttons.insert(button);
        let from_right = f64::from(bar.loc.x + bar.size.w) - self.pointer_location.x;
        if let Some(control) = crate::appearance::TitlebarControl::from_right_distance(from_right) {
            use crate::appearance::TitlebarControl;
            match control {
                TitlebarControl::Close => {
                    self.apply_window_action(&window, crate::workspaces::WindowAction::Close)
                }
                TitlebarControl::Maximize => self.set_window_maximized(&window, !maximized),
                TitlebarControl::Float => {
                    let _ = self.toggle_floating_focused();
                }
                TitlebarControl::Minimize => {
                    self.apply_window_action(&window, crate::workspaces::WindowAction::Minimize)
                }
            }
        } else if let Some(id) = self.window_id(&window) {
            let double = self
                .titlebar_click
                .is_some_and(|(previous, when, location)| {
                    previous == id
                        && time.wrapping_sub(when) <= 400
                        && (location.x - self.pointer_location.x).abs() <= 5.0
                        && (location.y - self.pointer_location.y).abs() <= 5.0
                });
            self.titlebar_click = if double {
                None
            } else {
                Some((id, time, self.pointer_location))
            };
            if double {
                self.set_window_maximized(&window, !maximized);
            } else {
                let _ = self.begin_window_interaction(WindowTarget::Window(id), false, None);
            }
        }
        self.request_repaint();
        true
    }

    pub fn x11_grab_button_matches(&self, button: u32) -> bool {
        let code = match button {
            1 => 0x110,
            2 => 0x112,
            3 => 0x111,
            _ => return false,
        };
        self.pointer
            .grab_start_data()
            .is_some_and(|start| start.button == code && self.pressed_buttons.contains(&code))
    }

    pub fn start_window_grab(
        &mut self,
        window: Window,
        serial: Option<Serial>,
        edges: Option<ResizeEdges>,
    ) {
        if serial.is_some_and(|serial| !self.pointer.has_grab(serial))
            || self.interaction.is_some()
            || self.pressed_buttons.len() != 1
        {
            return;
        }
        let Some(start) = self.pointer.grab_start_data() else {
            return;
        };
        if !self.pressed_buttons.contains(&start.button) {
            return;
        }
        let root = start.focus.as_ref().and_then(|(surface, _)| {
            std::iter::successors(
                Some(surface.clone()),
                smithay::wayland::compositor::get_parent,
            )
            .last()
        });
        if root != window.wl_surface().map(|surface| surface.into_owned()) {
            return;
        }
        let Ok(operation) = self.window_operation(window, edges.is_some(), edges) else {
            return;
        };
        self.install_interaction(
            operation,
            Some(start.button),
            start,
            serial.unwrap_or_else(|| SERIAL_COUNTER.next_serial()),
        );
    }

    fn window_operation(
        &self,
        window: Window,
        resize: bool,
        edges: Option<ResizeEdges>,
    ) -> Result<Operation, DispatchError> {
        let placement = self
            .workspace_layout(self.active_workspace)
            .into_iter()
            .find(|entry| entry.0 == window && entry.5)
            .ok_or(DispatchError::Interaction(
                "window must be visible on the active workspace",
            ))?;
        if placement.2 || placement.3 {
            return Err(DispatchError::Interaction(
                "restore maximized/fullscreen window before dragging",
            ));
        }
        if let Some(geometry) = self.floating_geometry(&window) {
            return Ok(Operation::Floating {
                window,
                geometry,
                edges: resize.then(|| {
                    edges.unwrap_or_else(|| ResizeEdges::nearest(geometry, self.pointer_location))
                }),
            });
        }
        if !resize {
            return Ok(Operation::Tile {
                window,
                geometry: placement.1,
                destination: None,
            });
        }
        let context = self.split_context().ok_or(DispatchError::Interaction(
            "there is no tiled split to resize",
        ))?;
        let offset = self.pointer_location.x
            - f64::from(
                context.area.loc.x
                    + crate::workspaces::master_width(
                        context.area.size.w,
                        self.master_ratio(self.active_workspace),
                    ),
            );
        Ok(Operation::Master {
            context,
            initial: self.master_override(),
            offset,
        })
    }

    pub(crate) fn begin_window_interaction(
        &mut self,
        target: WindowTarget,
        resize: bool,
        edge: Option<PublicEdge>,
    ) -> Result<(), DispatchError> {
        if !self.can_start_interaction() {
            return Err(DispatchError::Interaction(
                "another interaction or shell surface owns input",
            ));
        }
        if target == WindowTarget::Pointer && self.layer_under_pointer(true).is_some() {
            return Err(DispatchError::Interaction("shell surface is under pointer"));
        }
        let window = match target {
            WindowTarget::Focused => self
                .focused_window()
                .ok_or(DispatchError::NoFocusedWindow)?,
            WindowTarget::Pointer => self
                .input_window_at_pointer()
                .map(|hit| hit.0)
                .or_else(|| self.floating_edge_under_pointer().map(|hit| hit.0))
                .ok_or(DispatchError::Interaction("no window under pointer"))?,
            WindowTarget::Window(id) => self
                .capture_window(id)
                .ok_or(DispatchError::UnknownWindow(id))?,
        };
        let operation =
            self.window_operation(window.clone(), resize, edge.map(ResizeEdges::from_public))?;
        self.focus_managed_window(&window);
        let button =
            (self.pressed_buttons.len() == 1).then(|| *self.pressed_buttons.iter().next().unwrap());
        self.start_compositor_interaction(operation, button);
        Ok(())
    }

    pub(crate) fn can_start_interaction(&self) -> bool {
        self.host_focused
            && self.interaction.is_none()
            && !self.pointer.is_grabbed()
            && !self.keyboard.is_grabbed()
            && self.exclusive_layer_focus().is_none()
            && !self.overview_visible
            && !self.overview_mapped()
            && !self.screenshot.intercepting_pointer()
            && self.pressed_buttons.len() <= 1
    }

    pub(crate) fn start_compositor_interaction(
        &mut self,
        operation: Operation,
        button: Option<u32>,
    ) {
        let start = GrabStartData {
            focus: None,
            button: button.unwrap_or(0x110),
            location: self.pointer_location,
        };
        let serial = SERIAL_COUNTER.next_serial();
        self.install_interaction(operation, button, start, serial);
        if let Some(button) = button {
            let pointer = self.pointer.clone();
            pointer.button(
                self,
                &ButtonEvent {
                    serial,
                    time: 0,
                    button,
                    state: ButtonState::Pressed,
                },
            );
            pointer.frame(self);
        }
    }

    fn install_interaction(
        &mut self,
        operation: Operation,
        button: Option<u32>,
        start: GrabStartData<Self>,
        serial: Serial,
    ) {
        self.pending_modifier = None;
        if let Some(button) = button {
            self.swallowed_buttons.insert(button);
        }
        self.split_drag_active = matches!(
            operation,
            Operation::Master { .. } | Operation::Stack { .. }
        );
        if let Operation::Floating {
            window,
            edges: Some(_),
            ..
        } = &operation
        {
            set_resizing(window, true);
        }
        self.interaction = Some(Interaction {
            operation,
            start: start.location,
            button,
            workspace: self.active_workspace,
            area: self.usable_area(),
            output: self.output_size,
            delta: (0, 0).into(),
            moved: false,
        });
        self.pointer
            .clone()
            .set_grab(self, WindowGrab { start }, serial, Focus::Clear);
        self.update_interaction_cursor();
        self.request_repaint();
    }

    pub(crate) fn floating_edge_under_pointer(&self) -> Option<(Window, ResizeEdges)> {
        if self.layer_under_pointer(true).is_some() || self.exclusive_layer_focus().is_some() {
            return None;
        }
        let hit_window = self.input_window_at_pointer().map(|hit| hit.0);
        for window in self.space.elements().rev() {
            let Some(rect) = self.floating_geometry(window) else {
                // A visible higher window occludes resize handles below it.
                if hit_window.as_ref() == Some(window) {
                    return None;
                }
                continue;
            };
            let (outer, _, _) = self.input_shape(window, rect);
            let hit = Geometry::new(
                outer.loc - Point::from((5, 5)),
                (outer.size.w + 10, outer.size.h + 10).into(),
            );
            if !hit.to_f64().contains(self.pointer_location) {
                continue;
            }
            if rect.to_f64().contains(self.pointer_location) {
                return None;
            }
            // A popup outside its parent keeps its own input.
            if let Some((surface, _)) = window.surface_under(
                self.pointer_location - (rect.loc - window.geometry().loc).to_f64(),
                smithay::desktop::WindowSurfaceType::ALL,
            ) {
                let root =
                    std::iter::successors(Some(surface), smithay::wayland::compositor::get_parent)
                        .last();
                if root != Self::window_surface(window) {
                    return None;
                }
            }
            let p = self.pointer_location;
            let corner_x = (rect.size.w / 3).clamp(1, 12);
            let corner_y = (rect.size.h / 3).clamp(1, 12);
            return Some((
                window.clone(),
                ResizeEdges {
                    left: p.x < f64::from(rect.loc.x + corner_x),
                    right: p.x >= f64::from(rect.loc.x + rect.size.w - corner_x),
                    top: p.y < f64::from(rect.loc.y + corner_y),
                    bottom: p.y >= f64::from(rect.loc.y + rect.size.h - corner_y),
                },
            ));
        }
        None
    }

    pub(crate) fn try_start_edge_resize(&mut self, button: u32, state: ButtonState) -> bool {
        if button != 0x110 || state != ButtonState::Pressed || !self.can_start_interaction() {
            return false;
        }
        let Some((window, edges)) = self.floating_edge_under_pointer() else {
            return false;
        };
        let Ok(operation) = self.window_operation(window.clone(), true, Some(edges)) else {
            return false;
        };
        self.focus_managed_window(&window);
        self.start_compositor_interaction(operation, Some(button));
        true
    }

    pub(crate) fn update_interaction_cursor(&mut self) {
        let icon = if let Some(drag) = &self.interaction {
            Some(match &drag.operation {
                Operation::Floating {
                    edges: Some(edges), ..
                } => edges.cursor(),
                Operation::Floating { .. } | Operation::Tile { .. } => CursorIcon::Grabbing,
                Operation::Master { .. } => CursorIcon::ColResize,
                Operation::Stack { .. } => CursorIcon::RowResize,
            })
        } else if self.can_start_interaction() {
            self.floating_edge_under_pointer()
                .map(|(_, edges)| edges.cursor())
                .or_else(|| {
                    self.stack_divider_under_pointer()
                        .map(|_| CursorIcon::RowResize)
                })
                .or_else(|| self.split_under_pointer().map(|_| CursorIcon::ColResize))
        } else {
            None
        };
        self.cursor.set_interaction_override(icon);
    }

    fn interaction_motion(&mut self, location: Point<f64, Logical>) {
        let Some(mut drag) = self.interaction.take() else {
            return;
        };
        if drag.workspace != self.active_workspace
            || drag.area != self.usable_area()
            || drag.output != self.output_size
        {
            self.interaction = Some(drag);
            self.cancel_window_interaction();
            return;
        }
        drag.delta = (location - drag.start).to_i32_round();
        drag.moved |= drag.delta.x.abs().max(drag.delta.y.abs()) >= 5;
        match &mut drag.operation {
            Operation::Floating {
                window,
                geometry,
                edges,
            } => {
                if self.floating_geometry(window).is_none() {
                    self.interaction = Some(drag);
                    self.cancel_window_interaction();
                    return;
                }
                if drag.moved {
                    let rect = if let Some(edges) = edges {
                        let (min, max) = Self::window_constraints(window);
                        {
                            let bounds =
                                self.floating_placement(Geometry::from_size(self.output_size));
                            let local = Geometry::new(geometry.loc - bounds.loc, geometry.size);
                            let mut resized =
                                resize_geometry(local, drag.delta, *edges, bounds.size, min, max);
                            resized.loc += bounds.loc;
                            resized
                        }
                    } else {
                        Geometry::new(geometry.loc + drag.delta, geometry.size)
                    };
                    self.set_floating_geometry(window, rect);
                }
            }
            Operation::Tile {
                window,
                destination,
                ..
            } => {
                let slots = self.tiled_slots();
                if !slots.iter().any(|(w, _)| w == window) {
                    self.interaction = Some(drag);
                    self.cancel_window_interaction();
                    return;
                }
                let occluded = self.layer_under_pointer(true).is_some()
                    || self
                        .input_window_at_pointer()
                        .is_some_and(|(hit, _)| self.floating_geometry(&hit).is_some());
                *destination = slots
                    .into_iter()
                    .find(|(w, rect)| !occluded && w != window && rect.to_f64().contains(location))
                    .map(|(w, _)| w);
                self.request_repaint();
            }
            Operation::Master {
                context, offset, ..
            } => {
                if self.split_context().as_ref() != Some(context) {
                    self.interaction = Some(drag);
                    self.cancel_window_interaction();
                    return;
                }
                let width = location.x - *offset - f64::from(context.area.loc.x);
                let ratio = (width * 10000.0 / f64::from(context.area.size.w))
                    .round()
                    .clamp(1000.0, 9000.0) as u16;
                self.apply_master_ratio(Some(ratio));
            }
            Operation::Stack {
                context,
                weights,
                pair,
                upper,
                lower,
            } => {
                if self.split_context().as_ref() != Some(context) {
                    self.interaction = Some(drag);
                    self.cancel_window_interaction();
                    return;
                }
                let total_height = *upper + *lower;
                let minimum = 64.min(total_height / 2).max(1);
                let height = (*upper + drag.delta.y).clamp(minimum, total_height - minimum);
                let mut next = weights.clone();
                let total_weight = u64::from(weights[*pair].1) + u64::from(weights[*pair + 1].1);
                let first = (total_weight * height as u64 / total_height as u64)
                    .clamp(1, total_weight - 1) as u32;
                next[*pair].1 = first;
                next[*pair + 1].1 = (total_weight - u64::from(first)) as u32;
                self.apply_stack_weights(&next);
            }
        }
        self.interaction = Some(drag);
    }

    pub(crate) fn cancel_window_interaction(&mut self) {
        self.finish_window_interaction(false);
    }

    pub(crate) fn finish_window_interaction(&mut self, commit: bool) {
        let Some(drag) = self.interaction.take() else {
            return;
        };
        match drag.operation {
            Operation::Floating {
                window,
                geometry,
                edges,
            } => {
                if !commit && drag.workspace == self.active_workspace {
                    self.set_floating_geometry(&window, geometry);
                }
                if edges.is_some() {
                    set_resizing(&window, false);
                }
            }
            Operation::Tile {
                window,
                destination: Some(destination),
                ..
            } if commit && drag.moved => self.swap_tiles(&window, &destination),
            Operation::Master { initial, .. }
                if !commit && drag.workspace == self.active_workspace =>
            {
                self.apply_master_ratio(initial)
            }
            Operation::Stack { weights, .. }
                if !commit && drag.workspace == self.active_workspace =>
            {
                self.apply_stack_weights(&weights)
            }
            _ => {}
        }
        self.split_drag_active = false;
        self.cursor.set_split_override(false);
        self.cursor.set_interaction_override(None);
        self.request_repaint();
    }

    pub(crate) fn finish_interaction_key(&mut self, commit: bool) {
        self.finish_window_interaction(commit);
        self.release_pointer_buttons();
        self.refresh_pointer(0);
    }
}

fn set_resizing(window: &Window, resizing: bool) {
    if let Some(surface) = window.toplevel() {
        surface.with_pending_state(|state| {
            if resizing {
                state.states.set(xdg_toplevel::State::Resizing);
            } else {
                state.states.unset(xdg_toplevel::State::Resizing);
            }
        });
        surface.send_pending_configure();
    }
}

struct WindowGrab {
    start: GrabStartData<Villain>,
}
fn resize_geometry(
    start: Geometry,
    delta: Point<i32, Logical>,
    edges: ResizeEdges,
    output: smithay::utils::Size<i32, Logical>,
    min: smithay::utils::Size<i32, Logical>,
    max: smithay::utils::Size<i32, Logical>,
) -> Geometry {
    let axis = |position: i32,
                size: i32,
                delta: i32,
                negative: bool,
                positive: bool,
                limit: i32,
                minimum: i32,
                maximum: i32| {
        if !negative && !positive {
            return (position, size);
        }
        let available = if negative {
            position.saturating_add(size)
        } else {
            limit.saturating_sub(position)
        }
        .max(1);
        let upper = if maximum > 0 {
            maximum.min(available)
        } else {
            available
        }
        .max(1);
        let lower = minimum.max(1).min(upper);
        let requested = if negative {
            size.saturating_sub(delta)
        } else {
            size.saturating_add(delta)
        };
        let resized = requested.clamp(lower, upper);
        (
            if negative {
                position + size - resized
            } else {
                position
            },
            resized,
        )
    };
    let (x, w) = axis(
        start.loc.x,
        start.size.w,
        delta.x,
        edges.left,
        edges.right,
        output.w,
        min.w,
        max.w,
    );
    let (y, h) = axis(
        start.loc.y,
        start.size.h,
        delta.y,
        edges.top,
        edges.bottom,
        output.h,
        min.h,
        max.h,
    );
    Geometry::new((x, y).into(), (w, h).into())
}

impl PointerGrab<Villain> for WindowGrab {
    fn motion(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        _focus: PointerFocus,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        data.interaction_motion(event.location);
        if data.interaction.is_none() {
            // The backend clears physical and Smithay button bookkeeping after
            // leaving the pointer lock, while this grab still owns client input.
            data.interaction_cleanup_pending = true;
        }
    }
    fn relative_motion(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        focus: PointerFocus,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }
    fn button(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if event.state == ButtonState::Pressed {
            data.swallowed_buttons.insert(event.button);
        }
        let finish = data.interaction.as_ref().is_some_and(|drag| {
            if let Some(button) = drag.button {
                event.button == button && event.state == ButtonState::Released
            } else {
                event.button == 0x110 && event.state == ButtonState::Released
            }
        });
        if finish {
            data.finish_window_interaction(true);
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }
    fn axis(
        &mut self,
        _data: &mut Villain,
        _handle: &mut PointerInnerHandle<'_, Villain>,
        _details: AxisFrame,
    ) {
    }
    fn frame(&mut self, data: &mut Villain, handle: &mut PointerInnerHandle<'_, Villain>) {
        handle.frame(data);
    }
    fn start_data(&self) -> &GrabStartData<Villain> {
        &self.start
    }
    fn unset(&mut self, data: &mut Villain) {
        data.cancel_window_interaction();
    }
    fn gesture_swipe_begin(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }
    fn gesture_swipe_update(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }
    fn gesture_swipe_end(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }
    fn gesture_pinch_begin(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }
    fn gesture_pinch_update(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }
    fn gesture_pinch_end(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }
    fn gesture_hold_begin(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }
    fn gesture_hold_end(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::constrain_geometry;
    #[test]
    fn window_menu_rows_cover_exactly_four_visible_rows() {
        let rect = Rectangle::new((20, 30).into(), (200, 120).into());
        assert_eq!(window_menu_row(rect, (20.0, 30.0).into()), Some(0));
        assert_eq!(window_menu_row(rect, (219.0, 59.9).into()), Some(0));
        assert_eq!(window_menu_row(rect, (20.0, 60.0).into()), Some(1));
        assert_eq!(window_menu_row(rect, (20.0, 149.9).into()), Some(3));
        assert_eq!(window_menu_row(rect, (20.0, 150.0).into()), None);
        assert_eq!(window_menu_row(rect, (220.0, 60.0).into()), None);
    }
    #[test]
    fn resizing_to_output_bounds_keeps_the_opposite_edge_fixed() {
        let initial = Geometry::new((100, 100).into(), (300, 200).into());
        assert_eq!(
            resize_geometry(
                initial,
                (-500, -500).into(),
                ResizeEdges::from_x11(ResizeEdge::TopLeft),
                (800, 600).into(),
                (1, 1).into(),
                (0, 0).into()
            ),
            Geometry::new((0, 0).into(), (400, 300).into())
        );
        assert_eq!(
            resize_geometry(
                initial,
                (5000, 5000).into(),
                ResizeEdges::from_x11(ResizeEdge::BottomRight),
                (800, 600).into(),
                (1, 1).into(),
                (0, 0).into()
            ),
            Geometry::new((100, 100).into(), (700, 500).into())
        );
    }
    #[test]
    fn resizing_from_top_left_preserves_opposite_corner_at_minimum() {
        let initial = Rectangle::new((100, 100).into(), (300, 200).into());
        let rect = resize_geometry(
            initial,
            (500, 500).into(),
            ResizeEdges::from_x11(ResizeEdge::TopLeft),
            (800, 600).into(),
            (120, 80).into(),
            (0, 0).into(),
        );
        assert_eq!(rect, Rectangle::new((280, 220).into(), (120, 80).into()));
    }
    #[test]
    fn fixed_size_and_output_limits_survive_resize_requests() {
        let initial = Rectangle::new((10, 10).into(), (240, 180).into());
        let rect = resize_geometry(
            initial,
            (5000, 5000).into(),
            ResizeEdges::from_x11(ResizeEdge::BottomRight),
            (800, 600).into(),
            (240, 180).into(),
            (240, 180).into(),
        );
        assert_eq!(rect, initial);
        assert_eq!(
            constrain_geometry(
                Rectangle::new((-100, 5000).into(), (2000, 2000).into()),
                (800, 600).into(),
                (0, 0).into(),
                (0, 0).into()
            ),
            Rectangle::from_size((800, 600).into())
        );
    }
}
