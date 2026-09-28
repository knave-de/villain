//! Output-owned desktop surfaces, independent of workspace membership.
use crate::{focus::KeyboardFocus, state::Villain};
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::{LayerSurface, WindowSurfaceType, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::shell::wlr_layer::{
        self, KeyboardInteractivity, Layer, WlrLayerShellHandler, WlrLayerShellState,
    },
};

pub struct ShellSurface {
    pub layer: LayerSurface,
    pub output: Output,
    pub mapped: bool,
}

impl WlrLayerShellHandler for Villain {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: wlr_layer::LayerSurface,
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = output
            .and_then(|o| Output::from_resource(&o))
            .filter(|o| self.space.outputs().any(|candidate| candidate == o))
            .or_else(|| self.space.outputs().next().cloned());
        let Some(output) = output else {
            surface.send_close();
            return;
        };
        self.shell_surfaces.push(ShellSurface {
            layer: LayerSurface::new(surface, namespace),
            output,
            mapped: false,
        });
    }

    fn layer_destroyed(&mut self, surface: wlr_layer::LayerSurface) {
        self.dismiss_layer_popups(surface.wl_surface());
        if self.shell_surfaces.iter().any(|entry| {
            entry.layer.layer_surface() == &surface
                && entry.layer.namespace() == "knave-shell-overview"
        }) {
            self.overview_panes.clear();
            self.request_repaint();
        }
        self.shell_surfaces.retain(|entry| {
            if entry.layer.layer_surface() == &surface {
                layer_map_for_output(&entry.output).unmap_layer(&entry.layer);
                false
            } else {
                true
            }
        });
        self.relayout_active_workspace();
    }
}

impl Villain {
    pub fn layer_commit(&mut self, surface: &WlSurface) -> bool {
        let Some(index) = self
            .shell_surfaces
            .iter()
            .position(|e| e.layer.wl_surface() == surface)
        else {
            return false;
        };
        let old_area = self.usable_area();
        let entry = &mut self.shell_surfaces[index];
        let mapped =
            with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
        let was_mapped = entry.mapped;
        {
            let mut map = layer_map_for_output(&entry.output);
            if was_mapped && !mapped {
                map.unmap_layer(&entry.layer);
            } else {
                // Arrange even the initial bufferless commit to negotiate its size.
                map.map_layer(&entry.layer)
                    .expect("layer belongs to its assigned output");
                map.arrange();
                if !smithay::wayland::compositor::with_states(surface, |states| {
                    states
                        .data_map
                        .get::<wlr_layer::LayerSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .initial_configure_sent
                }) {
                    entry.layer.layer_surface().send_configure();
                }
                if !mapped {
                    map.unmap_layer(&entry.layer);
                }
            }
        }
        entry.mapped = mapped;
        if !mapped && entry.layer.namespace() == "knave-shell-overview" {
            self.overview_panes.clear();
        }
        if was_mapped && !mapped {
            self.dismiss_layer_popups(surface);
        }
        if mapped != was_mapped || self.usable_area() != old_area {
            self.relayout_active_workspace();
        } else {
            self.refresh_pointer(0);
        }
        if !self.keyboard.is_grabbed()
            && let Some(focus) = self.exclusive_layer_focus()
            && self.keyboard.current_focus().as_ref() != Some(&focus)
        {
            self.release_pointer_buttons();
            self.focus_layer(focus);
        }
        self.request_repaint();
        true
    }

    fn dismiss_layer_popups(&mut self, surface: &WlSurface) {
        for (popup, _) in smithay::desktop::PopupManager::popups_for_surface(surface) {
            let _ = smithay::desktop::PopupManager::dismiss_popup(surface, &popup);
        }
        if self
            .keyboard
            .grab_start_data()
            .is_some_and(|start| start.focus == Some(KeyboardFocus::Wayland(surface.clone())))
        {
            self.keyboard.clone().unset_grab(self);
            self.release_pointer_buttons();
        }
    }

    pub fn arrange_layers(&mut self) {
        for output in self.space.outputs() {
            layer_map_for_output(output).arrange();
        }
    }

    pub fn usable_area(&self) -> Rectangle<i32, Logical> {
        let mut area = self
            .space
            .outputs()
            .next()
            .map(|o| layer_map_for_output(o).non_exclusive_zone())
            .unwrap_or_else(|| Rectangle::from_size(self.output_size));
        area.size.w = area.size.w.max(1);
        area.size.h = area.size.h.max(1);
        area
    }

    pub fn layer_surface_visible(&self, surface: &WlSurface) -> bool {
        self.shell_surfaces.iter().any(|e| {
            e.mapped
                && (e.layer.wl_surface() == surface
                    || layer_map_for_output(&e.output)
                        .layer_for_surface(surface, WindowSurfaceType::ALL)
                        .is_some())
        })
    }

    pub fn exclusive_layer_focus(&self) -> Option<KeyboardFocus> {
        if !self.host_focused {
            return None;
        }
        for kind in [Layer::Overlay, Layer::Top] {
            if let Some(entry) = self.shell_surfaces.iter().rev().find(|e| {
                e.mapped
                    && e.layer.layer() == kind
                    && e.layer.cached_state().keyboard_interactivity
                        == KeyboardInteractivity::Exclusive
            }) {
                return Some(KeyboardFocus::Wayland(entry.layer.wl_surface().clone()));
            }
        }
        None
    }

    pub fn layer_under_pointer(
        &self,
        upper: bool,
    ) -> Option<(WlSurface, Point<f64, Logical>, Option<KeyboardFocus>)> {
        let kinds = if upper {
            [Layer::Overlay, Layer::Top]
        } else {
            [Layer::Bottom, Layer::Background]
        };
        for kind in kinds {
            for output in self.space.outputs() {
                let map = layer_map_for_output(output);
                for layer in map.layers_on(kind).rev() {
                    let origin = map.layer_geometry(layer)?.loc.to_f64();
                    if let Some((surface, offset)) =
                        layer.surface_under(self.pointer_location - origin, WindowSurfaceType::ALL)
                    {
                        let keyboard = layer
                            .can_receive_keyboard_focus()
                            .then(|| KeyboardFocus::Wayland(layer.wl_surface().clone()));
                        return Some((surface, origin + offset.to_f64(), keyboard));
                    }
                }
            }
        }
        None
    }

    pub fn focus_layer(&mut self, focus: KeyboardFocus) {
        if self.keyboard.current_focus().as_ref() != Some(&focus) {
            for window in self.space.elements() {
                Self::set_activated(window, false);
            }
            self.keyboard
                .clone()
                .set_focus(self, Some(focus), SERIAL_COUNTER.next_serial());
        }
    }
}
smithay::delegate_layer_shell!(Villain);

#[cfg(test)]
#[path = "layer_tests.rs"]
mod tests;
