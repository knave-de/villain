//! Real Wayland protocol requests against a private, headless Villain instance.
use super::*;
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use std::{
    collections::HashMap,
    os::unix::net::UnixStream,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_compositor, wl_registry, wl_surface},
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Default, Clone, Debug)]
struct Configure {
    width: i32,
    height: i32,
    fullscreen: bool,
    tiled: bool,
}
#[derive(Default)]
struct Client {
    configures: HashMap<usize, Configure>,
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        _: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
        }
    }
}
impl Dispatch<xdg_toplevel::XdgToplevel, usize> for Client {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        id: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure {
            width,
            height,
            states,
        } = event
        {
            let states: Vec<_> = states
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| u32::from_ne_bytes(*bytes))
                .collect();
            state.configures.insert(
                *id,
                Configure {
                    width,
                    height,
                    fullscreen: states.contains(&(xdg_toplevel::State::Fullscreen as u32)),
                    tiled: states.contains(&(xdg_toplevel::State::TiledLeft as u32)),
                },
            );
        }
    }
}
delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_surface::WlSurface);

type Inspect = Box<dyn FnOnce(&mut Villain) + Send>;
fn inspect(sender: &mpsc::Sender<Inspect>, f: impl FnOnce(&mut Villain) + Send + 'static) {
    let (done, receive) = mpsc::channel();
    sender
        .send(Box::new(move |state| {
            f(state);
            done.send(()).unwrap();
        }))
        .unwrap();
    receive.recv_timeout(Duration::from_secs(5)).unwrap();
}
fn find(state: &Villain, app: &str) -> WindowInfo {
    state
        .window_info()
        .into_iter()
        .find(|window| window.app_id == app)
        .unwrap()
}
fn mapped(state: &Villain, app: &str) -> bool {
    let info = find(state, app);
    state
        .workspace_layout(info.workspace.0 as usize - 1)
        .into_iter()
        .find(|(window, _, _, _, _)| {
            state.workspaces[info.workspace.0 as usize - 1]
                .windows
                .iter()
                .any(|entry| entry.id == info.id && entry.window == *window)
        })
        .is_some_and(|(window, _, _, _, visible)| {
            visible && state.space.element_location(&window).is_some()
        })
}

#[test]
#[ignore = "requires private Unix sockets and an isolated XDG_RUNTIME_DIR"]
fn wayland_floating_and_fullscreen_requests() {
    let mut event_loop = EventLoop::try_new().unwrap();
    let mut state = Villain::new(
        &mut event_loop,
        Display::new().unwrap(),
        crate::config::RuntimeConfig::load().unwrap(),
    );
    let (server, client) = UnixStream::pair().unwrap();
    state
        .display_handle
        .insert_client(server, Arc::new(crate::state::ClientState::default()))
        .unwrap();
    let (sender, requests) = mpsc::channel::<Inspect>();
    let thread = std::thread::spawn(move || {
        let conn = Connection::from_socket(client).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
        let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let mut client = Client::default();
        let create = |id, name: &str| {
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm.get_xdg_surface(&surface, &qh, ());
            let top = xdg.get_toplevel(&qh, id);
            top.set_app_id(name.into());
            (surface, xdg, top)
        };
        let settle = |queue: &mut wayland_client::EventQueue<Client>, client: &mut Client| {
            queue.roundtrip(client).unwrap();
            queue.roundtrip(client).unwrap();
        };
        let (a, _ax, at) = create(1, "primary");
        a.commit();
        let (b, _bx, bt) = create(2, "other");
        b.commit();
        settle(&mut queue, &mut client);
        inspect(&sender, |state| {
            assert!(find(state, "other").focused);
            assert!(!find(state, "primary").focused);
            // The pointer starts over the first tiled slot. Returning to the
            // workspace must still restore the remembered focus instead of
            // letting that stale hit-test win.
            state.switch_workspace(1);
            state.switch_workspace(0);
            state.refresh_pointer(0);
            assert!(find(state, "other").focused);
        });
        assert_eq!(
            (client.configures[&1].width, client.configures[&1].height),
            (400, 600)
        );
        assert!(client.configures[&1].tiled);
        let (dialog, _dx, dt) = create(3, "dialog");
        dt.set_parent(Some(&at));
        dialog.commit();
        let (fixed, _fx, ft) = create(4, "fixed");
        ft.set_min_size(240, 180);
        ft.set_max_size(240, 180);
        fixed.commit();
        settle(&mut queue, &mut client);
        inspect(&sender, |state| {
            assert!(find(state, "dialog").floating);
            assert!(find(state, "fixed").floating);
            assert!(mapped(state, "primary") && mapped(state, "other") && mapped(state, "dialog"));
        });
        assert_eq!(
            (client.configures[&4].width, client.configures[&4].height),
            (240, 180)
        );
        assert!(!client.configures[&3].tiled);
        let dialog_size = (client.configures[&3].width, client.configures[&3].height);
        at.set_fullscreen(None);
        settle(&mut queue, &mut client);
        assert!(client.configures[&1].fullscreen && !client.configures[&1].tiled);
        client.configures.remove(&1);
        at.set_fullscreen(None);
        settle(&mut queue, &mut client);
        assert!(
            client.configures[&1].fullscreen,
            "repeated request needs a configure response"
        );
        assert_eq!(
            (client.configures[&1].width, client.configures[&1].height),
            (800, 600)
        );
        inspect(&sender, |state| {
            assert!(mapped(state, "primary") && mapped(state, "dialog"));
            assert!(!mapped(state, "other") && !mapped(state, "fixed"));
            assert_eq!(state.workspace_info()[0].visible_window_count, 2);
        });
        // A floating window restores its old size after fullscreen.
        dt.set_fullscreen(None);
        settle(&mut queue, &mut client);
        assert!(client.configures[&3].fullscreen);
        assert!(!client.configures[&1].fullscreen);
        inspect(&sender, |state| {
            let id = find(state, "primary").id;
            let parent = state.workspaces[0]
                .windows
                .iter()
                .find(|entry| entry.id == id)
                .unwrap()
                .window
                .clone();
            state.apply_window_action(&parent, WindowAction::Minimize);
            assert!(mapped(state, "other"));
            assert!(!mapped(state, "dialog"));
            assert_eq!(state.focus_window(find(state, "dialog").id), Some(false));
            assert!(state.restore_last_minimized_window());
            assert!(mapped(state, "dialog"));
        });
        dt.unset_fullscreen();
        settle(&mut queue, &mut client);
        assert_eq!(
            (client.configures[&3].width, client.configures[&3].height),
            dialog_size
        );
        assert!(!client.configures[&3].fullscreen);
        assert_eq!(
            (client.configures[&1].width, client.configures[&1].height),
            (400, 600)
        );
        inspect(&sender, |state| {
            assert!(mapped(state, "other") && mapped(state, "fixed"))
        });
        // Relaxing committed fixed-size hints returns a standalone window to tiling.
        ft.set_min_size(0, 0);
        ft.set_max_size(0, 0);
        fixed.commit();
        settle(&mut queue, &mut client);
        inspect(&sender, |state| assert!(!find(state, "fixed").floating));
        // Fullscreen on an inactive workspace must configure without switching it.
        inspect(&sender, |state| state.switch_workspace(4));
        bt.set_fullscreen(None);
        settle(&mut queue, &mut client);
        assert!(client.configures[&2].fullscreen);
        inspect(&sender, |state| {
            assert_eq!(state.active_workspace, 4);
            assert!(find(state, "other").fullscreen);
            assert!(!mapped(state, "other"));
            state.switch_workspace(0);
            assert!(mapped(state, "other") && !mapped(state, "primary"));
            let id = find(state, "other").id;
            assert_eq!(state.focus_window(id), Some(true));
            assert!(state.minimize_focused_window());
            assert!(mapped(state, "primary"));
            assert!(state.restore_last_minimized_window());
            assert!(mapped(state, "other") && !mapped(state, "primary"));
            // An explicit user focus request makes the selected window reachable.
            let id = find(state, "primary").id;
            state.focus_window(id);
            assert!(mapped(state, "primary"));
            assert!(!find(state, "other").fullscreen);
            state.switch_workspace(4);
            state.switch_workspace(0);
            assert!(find(state, "primary").focused);
        });
        // Set a parent after creation while viewing a different workspace.
        inspect(&sender, |state| state.switch_workspace(4));
        let (late, _lx, lt) = create(5, "late-dialog");
        lt.set_parent(Some(&at));
        late.commit();
        settle(&mut queue, &mut client);
        inspect(&sender, |state| {
            assert_eq!(find(state, "late-dialog").workspace.0, 1);
            assert!(find(state, "late-dialog").floating);
            assert!(!mapped(state, "late-dialog"));
        });
        at.set_fullscreen(None);
        settle(&mut queue, &mut client);
        at.destroy();
        _ax.destroy();
        a.destroy();
        settle(&mut queue, &mut client);
        inspect(&sender, |state| {
            state.switch_workspace(0);
            assert!(mapped(state, "other"));
            assert!(!state.workspace_has_fullscreen(0));
            assert!(find(state, "other").focused);
        });
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while !thread.is_finished() {
        assert!(Instant::now() < deadline, "Wayland request test timed out");
        event_loop
            .dispatch(Duration::from_millis(2), &mut state)
            .unwrap();
        while let Ok(inspect) = requests.try_recv() {
            inspect(&mut state);
        }
        state.space.refresh();
        state.display_handle.flush_clients().unwrap();
    }
    thread.join().unwrap();
}
