//! The state shared by every event-loop callback.

use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    desktop::{Space, Window},
    input::{SeatHandler, SeatState, keyboard::KeyboardHandle},
    reexports::{
        calloop::{
            EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction,
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
        },
    },
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        cursor_shape::CursorShapeManagerState,
        output::OutputManagerState,
        selection::{
            data_device::{DataDeviceState, set_data_device_focus},
            ext_data_control,
            primary_selection::{PrimarySelectionState, set_primary_focus},
            wlr_data_control,
        },
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
        xwayland_shell::XWaylandShellState,
    },
    xwayland::X11Wm,
};

use crate::config::RuntimeConfig;
use crate::cursor::CursorState;
use crate::dispatch::Dispatch;
use crate::workspaces::Workspace;

/// All mutable compositor state lives here.
///
/// Keeping this in one ordinary struct is intentional. `calloop` passes a
/// mutable reference to it into callbacks, so the first version does not need
/// `Arc<Mutex<_>>` for its own state.
pub struct Villain {
    pub tty: Option<crate::tty::Tty>,
    pub winit: Option<crate::render::Winit>,
    pub dmabuf_state: smithay::wayland::dmabuf::DmabufState,
    pub display_handle: DisplayHandle,
    pub socket_name: OsString,
    pub start_time: Instant,
    pub loop_signal: LoopSignal,
    _ipc_server: crate::ipc::IpcServer,
    pub config: RuntimeConfig,

    /// The desktop plane: windows are mapped here and later rendered here.
    pub space: Space<Window>,

    // Protocol state is kept separate because each Smithay handler owns one
    // protocol's bookkeeping and exposes it through a trait implementation.
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: smithay::wayland::shell::wlr_layer::WlrLayerShellState,
    pub shell_surfaces: Vec<crate::layer_shell::ShellSurface>,
    pub popups: smithay::desktop::PopupManager,
    pub xwayland_shell_state: XWaylandShellState,
    pub xwm: Option<X11Wm>,
    pub xwayland_display: Option<u32>,
    pub owns_session: bool,
    pub(crate) activation: crate::session::ActivationWorker,
    pub shm_state: ShmState,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub wlr_data_control_state: wlr_data_control::DataControlState,
    pub ext_data_control_state: ext_data_control::DataControlState,
    #[allow(dead_code)]
    pub cursor_shape_state: CursorShapeManagerState,
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,

    // XDG shell dispatch needs to know what a compositor considers a seat,
    // even before Villain creates real keyboard or pointer devices.
    pub seat_state: SeatState<Self>,
    pub seat: smithay::input::Seat<Self>,
    pub keyboard: KeyboardHandle<Self>,
    pub pointer: smithay::input::pointer::PointerHandle<Self>,
    pub pointer_location: smithay::utils::Point<f64, smithay::utils::Logical>,
    pub cursor: CursorState,
    pub workspaces: [Workspace; 10],
    pub unmanaged_x11_windows: Vec<crate::xwayland::UnmanagedWindow>,
    pub active_workspace: usize,
    pub next_window_id: u64,
    pub output_size: smithay::utils::Size<i32, smithay::utils::Logical>,
    pub children: Vec<(usize, std::process::Child)>,
    pub suppressed_keys: std::collections::HashSet<smithay::input::keyboard::Keycode>,
    pub pending_modifier: Option<(smithay::backend::input::Keycode, Dispatch)>,
    pub pending_focus_restore: bool,
    pub host_focused: bool,
    pub pressed_buttons: std::collections::HashSet<u32>,
    pub repaint_needed: bool,
    pub frame_clock: crate::frame_clock::FrameClock,
    cursor_timer_generation: u64,
    pub(crate) loop_handle: LoopHandle<'static, Self>,
}

impl Villain {
    pub fn new(
        event_loop: &mut EventLoop<'static, Self>,
        display: Display<Self>,
        config: RuntimeConfig,
    ) -> Self {
        let display_handle = display.handle();
        let socket_name = init_wayland_listener(display, event_loop);
        let ipc_server = crate::ipc::init(event_loop, &socket_name).expect("initialize IPC server");
        // GTK only exposes a default GdkSeat after it has both wl_seat and
        // wl_data_device_manager. Advertise the selection manager first so
        // clients can construct a complete seat as globals arrive.
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&display_handle);
        let wlr_data_control_state = wlr_data_control::DataControlState::new::<Self, _>(
            &display_handle,
            Some(&primary_selection_state),
            |_| true,
        );
        let ext_data_control_state = ext_data_control::DataControlState::new::<Self, _>(
            &display_handle,
            Some(&primary_selection_state),
            |_| true,
        );
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "villain");
        let keyboard = seat
            .add_keyboard(Default::default(), 200, 25)
            .expect("initialize keyboard");
        let pointer = seat.add_pointer();

        Self {
            tty: None,
            winit: None,
            dmabuf_state: smithay::wayland::dmabuf::DmabufState::new(),
            display_handle: display_handle.clone(),
            socket_name,
            start_time: Instant::now(),
            loop_signal: event_loop.get_signal(),
            _ipc_server: ipc_server,
            config,
            space: Space::default(),
            compositor_state: CompositorState::new::<Self>(&display_handle),
            layer_shell_state: smithay::wayland::shell::wlr_layer::WlrLayerShellState::new::<Self>(
                &display_handle,
            ),
            shell_surfaces: Vec::new(),
            popups: Default::default(),
            // Advertise only policy that Villain currently implements. Close
            // is a compositor-to-client event, not a WM capability.
            xdg_shell_state: XdgShellState::new_with_capabilities::<Self>(
                &display_handle,
                [
                    xdg_toplevel::WmCapabilities::Minimize,
                    xdg_toplevel::WmCapabilities::Fullscreen,
                ],
            ),
            xwayland_shell_state: XWaylandShellState::new::<Self>(&display_handle),
            xwm: None,
            xwayland_display: None,
            owns_session: false,
            activation: crate::session::ActivationWorker::new(),
            shm_state: ShmState::new::<Self>(&display_handle, vec![]),
            data_device_state,
            primary_selection_state,
            wlr_data_control_state,
            ext_data_control_state,
            cursor_shape_state: CursorShapeManagerState::new::<Self>(&display_handle),
            output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(&display_handle),
            seat_state,
            seat,
            keyboard,
            pointer,
            pointer_location: (0.0, 0.0).into(),
            cursor: CursorState::new(),
            workspaces: std::array::from_fn(|_| Workspace::default()),
            unmanaged_x11_windows: Vec::new(),
            active_workspace: 0,
            next_window_id: 1,
            output_size: (800, 600).into(),
            children: Vec::new(),
            suppressed_keys: Default::default(),
            pending_modifier: None,
            pending_focus_restore: false,
            host_focused: true,
            pressed_buttons: Default::default(),
            repaint_needed: true,
            frame_clock: Default::default(),
            cursor_timer_generation: 0,
            loop_handle: event_loop.handle(),
        }
    }

    pub fn request_repaint(&mut self) {
        self.repaint_needed = true;
    }

    /// Schedule exactly one future wake-up for an animated server cursor.
    /// Incrementing the generation makes older one-shot timers harmless.
    pub fn schedule_cursor_frame(&mut self, delay: Option<std::time::Duration>) {
        self.cursor_timer_generation = self.cursor_timer_generation.wrapping_add(1);
        let generation = self.cursor_timer_generation;
        let Some(delay) = delay else {
            return;
        };
        let result =
            self.loop_handle
                .insert_source(Timer::from_duration(delay), move |_, _, state| {
                    if state.cursor_timer_generation == generation {
                        state.request_repaint();
                    }
                    TimeoutAction::Drop
                });
        if let Err(error) = result {
            tracing::warn!(%error, "could not schedule cursor animation frame");
        }
    }

    /// Called after each batch of input, Wayland, IPC, DRM, or timer events.
    pub fn render_if_needed(&mut self) {
        if !self.repaint_needed {
            return;
        }
        if self.tty.is_some() {
            crate::tty::render_if_needed(self);
        } else if let Some(winit) = self.winit.as_mut() {
            winit.request_redraw();
        }
    }
}

impl SeatHandler for Villain {
    type KeyboardFocus = crate::focus::KeyboardFocus;
    type PointerFocus = smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
    type TouchFocus = smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(
        &mut self,
        _seat: &smithay::input::Seat<Self>,
        image: smithay::input::pointer::CursorImageStatus,
    ) {
        self.cursor.set_image(image, self.start_time.elapsed());
        self.request_repaint();
    }

    fn focus_changed(
        &mut self,
        seat: &smithay::input::Seat<Self>,
        focused: Option<&Self::KeyboardFocus>,
    ) {
        let client = focused.and_then(|focus| {
            smithay::wayland::seat::WaylandFocus::wl_surface(focus)
                .and_then(|surface| surface.client())
        });
        set_data_device_focus(&self.display_handle, seat, client.clone());
        set_primary_focus(&self.display_handle, seat, client);
    }
}

impl smithay::wayland::tablet_manager::TabletSeatHandler for Villain {}

/// Data attached to each connected client.
///
/// Smithay needs this because compositor state is associated with a client,
/// not only with the global compositor state.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

fn init_wayland_listener(
    display: Display<Villain>,
    event_loop: &mut EventLoop<Villain>,
) -> OsString {
    let listening_socket = ListeningSocketSource::new_auto().expect("create Wayland socket");
    let socket_name = listening_socket.socket_name().to_os_string();
    let loop_handle = event_loop.handle();

    loop_handle
        .insert_source(listening_socket, move |client_stream, _, state| {
            state
                .display_handle
                .insert_client(client_stream, Arc::new(ClientState::default()))
                .expect("insert Wayland client");
        })
        .expect("register Wayland socket");

    loop_handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            move |_, display, state| {
                // The Display is owned by this event source for the lifetime
                // of the event loop. calloop gives us a mutable source value.
                unsafe { display.get_mut().dispatch_clients(state) }?;
                Ok(PostAction::Continue)
            },
        )
        .expect("register Wayland display");

    socket_name
}
