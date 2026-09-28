//! Compositor-owned pointer grab for the master/stack boundary.
use crate::{
    state::Villain,
    workspaces::{SplitContext, master_width},
};
use smithay::{
    backend::input::ButtonState,
    input::pointer::*,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};
type PointerFocus = Option<(WlSurface, Point<f64, Logical>)>;

impl Villain {
    pub(crate) fn try_start_split_drag(
        &mut self,
        button: u32,
        state: ButtonState,
        time: u32,
    ) -> bool {
        if button != 0x110 || state != ButtonState::Pressed || self.pressed_buttons.len() != 1 {
            return false;
        }
        let Some(context) = self.split_under_pointer() else {
            return false;
        };
        let boundary = context.area.loc.x
            + master_width(
                context.area.size.w,
                self.master_ratio(self.active_workspace),
            );
        let grab = SplitGrab {
            start: GrabStartData {
                focus: None,
                button,
                location: self.pointer_location,
            },
            offset: self.pointer_location.x - f64::from(boundary),
            context,
        };
        let serial = SERIAL_COUNTER.next_serial();
        self.pending_modifier = None;
        self.split_drag_active = true;
        let pointer = self.pointer.clone();
        pointer.set_grab(self, grab, serial, Focus::Clear);
        pointer.button(
            self,
            &ButtonEvent {
                serial,
                time,
                button,
                state,
            },
        );
        pointer.frame(self);
        self.cursor.set_split_override(true);
        self.request_repaint();
        true
    }
}

struct SplitGrab {
    start: GrabStartData<Villain>,
    context: SplitContext,
    offset: f64,
}

impl PointerGrab<Villain> for SplitGrab {
    fn motion(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        _focus: PointerFocus,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        // A changed output, panel reservation, workspace or tiled membership
        // invalidates the drag rather than applying it to a different divider.
        if data.split_context().as_ref() != Some(&self.context) {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        let width = event.location.x - self.offset - f64::from(self.context.area.loc.x);
        let ratio = (width * 10000.0 / f64::from(self.context.area.size.w))
            .round()
            .clamp(1000.0, 9000.0) as u16;
        data.apply_master_ratio(Some(ratio));
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
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }
    fn axis(
        &mut self,
        data: &mut Villain,
        handle: &mut PointerInnerHandle<'_, Villain>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }
    fn frame(&mut self, data: &mut Villain, handle: &mut PointerInnerHandle<'_, Villain>) {
        handle.frame(data);
    }
    fn start_data(&self) -> &GrabStartData<Villain> {
        &self.start
    }
    fn unset(&mut self, data: &mut Villain) {
        data.split_drag_active = false;
        data.cursor.set_split_override(false);
        data.request_repaint();
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
