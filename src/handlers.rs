//! The smallest set of protocol handlers needed to show client windows.

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    reexports::wayland_server::{
        Client,
        protocol::{wl_buffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState, with_states},
        output::OutputHandler,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
    },
};
use smithay::{
    input::Seat,
    reexports::wayland_server::protocol::wl_seat,
    utils::Serial,
    wayland::selection::{
        SelectionHandler,
        data_device::{
            ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
        },
        ext_data_control,
        primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
        wlr_data_control,
    },
    xwayland::XWaylandClientData,
};
use std::os::fd::OwnedFd;

use crate::state::{ClientState, Villain};
use crate::workspaces::WindowAction;

impl CompositorHandler for Villain {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(state) = client.get_data::<ClientState>() {
            &state.compositor_state
        } else if let Some(state) = client.get_data::<XWaylandClientData>() {
            &state.compositor_state
        } else {
            unreachable!("Wayland client has no compositor state")
        }
    }

    fn commit(&mut self, surface: &WlSurface) {
        // This turns a client's wl_buffer commit into the state Smithay's
        // renderer can later inspect.
        on_commit_buffer_handler::<Self>(surface);
        self.popups.commit(surface);
        if let Some(smithay::desktop::PopupKind::Xdg(popup)) = self.popups.find_popup(surface) {
            if !popup.is_initial_configure_sent() {
                let _ = popup.send_configure();
            }
            self.request_repaint();
        }
        if self.layer_commit(surface) {
            return;
        }

        let root = std::iter::successors(Some(surface.clone()), |surface| {
            smithay::wayland::compositor::get_parent(surface)
        })
        .last()
        .unwrap();
        let visible_window = self
            .window_for_surface(&root)
            .is_some_and(|window| self.space.element_location(&window).is_some());
        if visible_window || self.layer_surface_visible(&root) || self.cursor.uses_surface(&root) {
            self.request_repaint();
        }

        // The first commit is the handshake: we tell the client which state
        // the compositor accepts, then the client can commit its first buffer.
        if let Some(window) = self.window_for_surface(surface) {
            // Update the window's bounding box from the newly committed
            // surface. Space uses this geometry to decide what gets rendered
            // on each output.
            window.on_commit();
            self.refresh_window_hints(&window);

            if let Some(toplevel) = window.toplevel() {
                let initial_configure_sent = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .expect("XDG toplevel data")
                        .lock()
                        .unwrap()
                        .initial_configure_sent
                });

                if !initial_configure_sent {
                    toplevel.send_configure();
                }
            }
            // A repaint can change the pointer's surface-local coordinates,
            // but it must not override focus chosen by a dispatcher.
            self.refresh_pointer_surface(0);
        }
    }
}

impl BufferHandler for Villain {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Villain {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SelectionHandler for Villain {
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        selection: smithay::wayland::selection::SelectionTarget,
        source: Option<smithay::wayland::selection::SelectionSource>,
        _seat: Seat<Self>,
    ) {
        let Some(xwm) = self.xwm.as_mut() else {
            return;
        };
        let mime_types = source.map(|source| source.mime_types());
        if let Err(error) = xwm.new_selection(selection, mime_types) {
            tracing::warn!(%error, ?selection, "could not export Wayland selection to XWayland");
        }
    }

    fn send_selection(
        &mut self,
        selection: smithay::wayland::selection::SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _user_data: &Self::SelectionUserData,
    ) {
        let Some(xwm) = self.xwm.as_mut() else {
            return;
        };
        if let Err(error) = xwm.send_selection(selection, mime_type, fd, self.loop_handle.clone()) {
            tracing::warn!(%error, ?selection, "could not transfer XWayland selection");
        }
    }
}

impl DataDeviceHandler for Villain {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl PrimarySelectionHandler for Villain {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl wlr_data_control::DataControlHandler for Villain {
    fn data_control_state(&self) -> &wlr_data_control::DataControlState {
        &self.wlr_data_control_state
    }
}

impl ext_data_control::DataControlHandler for Villain {
    fn data_control_state(&self) -> &ext_data_control::DataControlState {
        &self.ext_data_control_state
    }
}

impl ClientDndGrabHandler for Villain {}

impl ServerDndGrabHandler for Villain {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl OutputHandler for Villain {}

impl XdgShellHandler for Villain {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        tracing::info!("client created a toplevel surface");
        self.add_window(surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(&surface);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_window_maximized(&window, true);
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_window_maximized(&window, false);
        }
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            surface.with_pending_state(|state| state.fullscreen_output = output);
            self.set_window_fullscreen(&window, true);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_window_fullscreen(&window, false);
        }
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.refresh_window_hints(&window);
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        if self.seat.owns(&seat)
            && let Some(window) = self.window_for_surface(surface.wl_surface())
        {
            self.start_window_grab(window, Some(serial), None);
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) {
        if self.seat.owns(&seat)
            && let Some(edges) = crate::window_grab::ResizeEdges::from_xdg(edges)
            && let Some(window) = self.window_for_surface(surface.wl_surface())
        {
            self.start_window_grab(window, Some(serial), Some(edges));
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.apply_window_action(&window, WindowAction::Minimize);
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        let _ = self.popups.track_popup(surface.into());
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.popups.cleanup();
        self.refresh_pointer(0);
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        use smithay::desktop::{
            PopupKeyboardGrab, PopupKind, PopupPointerGrab, find_popup_root_surface,
        };
        use smithay::wayland::seat::WaylandFocus;
        if !self.seat.owns(&seat) {
            return;
        }
        let popup = PopupKind::Xdg(surface.clone());
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };
        // Layer popup grabs may only originate from the focused layer client.
        // Ordinary application popup policy remains separate.
        if !self.layer_surface_visible(&root) {
            return;
        }
        let focused = self
            .keyboard
            .current_focus()
            .and_then(|focus| focus.wl_surface().map(|s| s.into_owned()));
        if focused.as_ref() != Some(&root)
            && focused.as_ref() != surface.get_parent_surface().as_ref()
        {
            return;
        }
        if !self.pointer.has_grab(serial) && !self.keyboard.has_grab(serial) {
            return;
        }
        if let Ok(grab) = self.popups.grab_popup(
            crate::focus::KeyboardFocus::Wayland(root),
            popup,
            &self.seat,
            serial,
        ) {
            self.keyboard
                .clone()
                .set_focus(self, grab.current_grab(), serial);
            self.keyboard
                .clone()
                .set_grab(self, PopupKeyboardGrab::new(&grab), serial);
            self.pointer.clone().set_grab(
                self,
                PopupPointerGrab::new(&grab),
                serial,
                smithay::input::pointer::Focus::Keep,
            );
        }
    }
}

// Smithay's delegation macro connects the protocol objects to the handlers
// above. It is intentionally at the bottom: the implementations are easier to
// find before the generated dispatch glue.
smithay::delegate_compositor!(Villain);
smithay::delegate_cursor_shape!(Villain);
smithay::delegate_data_device!(Villain);
smithay::delegate_data_control!(Villain);
smithay::delegate_ext_data_control!(Villain);
smithay::delegate_output!(Villain);
smithay::delegate_primary_selection!(Villain);
smithay::delegate_seat!(Villain);
smithay::delegate_shm!(Villain);
smithay::delegate_xdg_shell!(Villain);

impl smithay::wayland::dmabuf::DmabufHandler for Villain {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &smithay::wayland::dmabuf::DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: smithay::wayland::dmabuf::ImportNotifier,
    ) {
        use smithay::backend::renderer::ImportDma;
        if self
            .tty
            .as_mut()
            .is_some_and(|tty| tty.renderer.import_dmabuf(&dmabuf, None).is_ok())
        {
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }
}
smithay::delegate_dmabuf!(Villain);
