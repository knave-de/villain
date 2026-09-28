//! The nested Winit backend and the one-frame render path.

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, AxisSource, Event, InputEvent, KeyState, PointerAxisEvent,
            PointerButtonEvent,
        },
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    desktop::space::render_output,
    input::pointer::{AxisFrame, ButtonEvent},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::Transform,
};

use crate::state::Villain;

pub struct Winit {
    backend: winit::WinitGraphicsBackend<GlesRenderer>,
    output: Output,
    damage: OutputDamageTracker,
    redraw_requested: bool,
}

impl Winit {
    pub fn request_redraw(&mut self) {
        if !self.redraw_requested {
            self.backend.window().request_redraw();
            self.redraw_requested = true;
        }
    }

    pub(crate) fn renderer(&mut self) -> &mut GlesRenderer {
        self.backend.renderer()
    }
}

/// Start a nested output and connect its events to the compositor loop.
pub fn init_winit(
    event_loop: &mut EventLoop<Villain>,
    state: &mut Villain,
) -> Result<(), Box<dyn std::error::Error>> {
    let (backend, winit_source) = winit::init()?;
    // Villain renders the cursor into its nested output. Winit only supplies
    // host pointer events and hides the host cursor while it is over this window.
    backend.window().set_cursor_visible(false);
    state.output_size = backend.window_size().to_logical(1);
    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        "villain-output".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Villain".into(),
            model: "Nested Winit output".into(),
        },
    );
    let _global = output.create_global::<Villain>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let damage = OutputDamageTracker::from_output(&output);
    state.winit = Some(Winit {
        backend,
        output,
        damage,
        redraw_requested: false,
    });
    event_loop
        .handle()
        .insert_source(winit_source, move |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                state.output_size = size.to_logical(1);
                state.overview_panes.clear();
                state.winit.as_ref().unwrap().output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
                state.relayout_active_workspace();
                state.request_repaint();
            }
            WinitEvent::Input(InputEvent::Keyboard { event }) => {
                crate::keybinds::handle_keyboard_event(state, event);
            }
            WinitEvent::Input(InputEvent::PointerMotionAbsolute { event }) => {
                state.pointer_location = (event.x(), event.y()).into();
                state.refresh_pointer(event.time_msec());
            }
            WinitEvent::Input(InputEvent::PointerButton { event }) => {
                match event.state() {
                    smithay::backend::input::ButtonState::Pressed => {
                        state.pressed_buttons.insert(event.button_code());
                    }
                    smithay::backend::input::ButtonState::Released => {
                        if !state.pressed_buttons.remove(&event.button_code()) {
                            return;
                        }
                    }
                }
                if state.try_start_split_drag(event.button_code(), event.state(), event.time_msec())
                {
                    return;
                }
                state.refresh_pointer_and_focus(event.time_msec());
                let pointer = state.pointer.clone();
                pointer.button(
                    state,
                    &ButtonEvent {
                        serial: smithay::utils::SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                        button: event.button_code(),
                        state: event.state(),
                    },
                );
                pointer.frame(state);
            }
            WinitEvent::Input(InputEvent::PointerAxis { event }) => {
                let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    let amount = event
                        .amount(axis)
                        .unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0);
                    frame = frame
                        .value(axis, amount)
                        .relative_direction(axis, event.relative_direction(axis));
                    if let Some(steps) = event.amount_v120(axis) {
                        frame = frame.v120(axis, steps as i32);
                    }
                    if event.source() == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                        frame = frame.stop(axis);
                    }
                }
                let pointer = state.pointer.clone();
                pointer.axis(state, frame);
                pointer.frame(state);
            }
            WinitEvent::Focus(focused) => {
                state.host_focused = focused;
                if !focused {
                    state.release_pointer_buttons();
                    let keyboard = state.keyboard.clone();
                    for code in keyboard.pressed_keys() {
                        keyboard.input::<(), _>(
                            state,
                            code,
                            KeyState::Released,
                            smithay::utils::SERIAL_COUNTER.next_serial(),
                            0,
                            |_, _, _| smithay::input::keyboard::FilterResult::Forward,
                        );
                    }
                    state.suppressed_keys.clear();
                    state.pending_modifier = None;
                }
                if focused {
                    state.refresh_pointer_surface(0);
                    state.restore_active_workspace_focus();
                } else {
                    state.refresh_pointer(0);
                }
            }
            WinitEvent::Redraw => {
                state.request_repaint();
                render_frame(state);
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => {}
        })?;

    Ok(())
}

fn render_frame(state: &mut Villain) {
    let Some(mut winit) = state.winit.take() else {
        return;
    };
    winit.redraw_requested = false;
    if !state.repaint_needed {
        state.winit = Some(winit);
        return;
    }
    state.repaint_needed = false;

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let age = winit.backend.buffer_age().unwrap_or(0);
        let (renderer, mut framebuffer) = winit.backend.bind()?;
        let now = state.start_time.elapsed();
        let cursor_elements = state
            .cursor
            .render_elements(renderer, state.pointer_location, now);
        let elements = crate::overview_render::elements(state, renderer, cursor_elements);
        let result = render_output::<_, crate::overview_render::OverviewRenderElement, _, _>(
            &winit.output,
            renderer,
            &mut framebuffer,
            1.0,
            age,
            [&state.space],
            &elements,
            &mut winit.damage,
            [0.08, 0.05, 0.12, 1.0],
        )?;
        let damage = result.damage.cloned();
        drop(framebuffer);
        if let Some(damage) = damage {
            winit.backend.submit(Some(&damage))?;
        }
        state.schedule_frame_callbacks(&winit.output);
        let delay = state.cursor.next_animation_delay(now);
        state.schedule_cursor_frame(delay);
        Ok(())
    })();
    if let Err(error) = result {
        tracing::error!(%error, "Winit render failed");
        state.repaint_needed = true;
    }
    state.winit = Some(winit);
}
