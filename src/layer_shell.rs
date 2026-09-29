//! Output-owned desktop surfaces, independent of workspace membership.
use crate::{focus::KeyboardFocus, state::Villain};
use knave_desktop_api::{OverviewPane, WindowId, WorkspaceId};
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::{LayerSurface, WindowSurfaceType, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size},
    wayland::shell::wlr_layer::{
        self, KeyboardInteractivity, Layer, WlrLayerShellHandler, WlrLayerShellState,
    },
};

pub struct ShellSurface {
    pub layer: LayerSurface,
    pub output: Output,
    pub mapped: bool,
}

fn pane_source_point(
    pane: &OverviewPane,
    source: Size<i32, Logical>,
    x: i32,
    y: i32,
) -> Option<(f64, f64)> {
    let local_x = i64::from(x) - i64::from(pane.x);
    let local_y = i64::from(y) - i64::from(pane.y);
    if local_x < 0
        || local_y < 0
        || local_x >= i64::from(pane.width)
        || local_y >= i64::from(pane.height)
        || source.w <= 0
        || source.h <= 0
    {
        return None;
    }
    let scale = (pane.width as f64 / source.w as f64).min(pane.height as f64 / source.h as f64);
    let offset_x = (pane.width as f64 - source.w as f64 * scale) / 2.0;
    let offset_y = (pane.height as f64 - source.h as f64 * scale) / 2.0;
    Some((
        (local_x as f64 - offset_x) / scale,
        (local_y as f64 - offset_y) / scale,
    ))
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
        if namespace == "knave-shell-overview" {
            if self.overview_cancel_pending
                || self
                    .shell_surfaces
                    .iter()
                    .any(|entry| entry.layer.namespace() == "knave-shell-overview")
            {
                self.overview_launch_pid = None;
                self.overview_cancel_pending = false;
                surface.send_close();
                return;
            }
            self.overview_launch_pid = None;
        }
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
    /// Resolve a shell click using the same output-to-pane fit as the renderer.
    pub fn overview_window_at(
        &self,
        workspace: WorkspaceId,
        x: i32,
        y: i32,
    ) -> Option<Option<WindowId>> {
        let pane = self
            .overview_panes
            .iter()
            .find(|pane| pane.workspace == workspace)?;
        let index = workspace.0.checked_sub(1)? as usize;
        let scene = self.workspace_preview_scene(index)?;
        let (source_x, source_y) = pane_source_point(pane, scene.output_size, x, y)?;
        let hit = scene.windows.iter().rev().find_map(|(window, rect)| {
            (source_x >= rect.loc.x as f64
                && source_y >= rect.loc.y as f64
                && source_x < (rect.loc.x + rect.size.w) as f64
                && source_y < (rect.loc.y + rect.size.h) as f64)
                .then(|| self.window_id(window))
                .flatten()
        });
        Some(hit)
    }

    pub fn toggle_overview(&mut self) -> std::io::Result<()> {
        let mut found = false;
        for entry in &self.shell_surfaces {
            if entry.layer.namespace() == "knave-shell-overview" {
                entry.layer.layer_surface().send_close();
                found = true;
            }
        }
        if found {
            return Ok(());
        }
        if self.overview_launch_pid.is_some() {
            self.overview_cancel_pending = !self.overview_cancel_pending;
            return Ok(());
        }
        self.spawn(vec!["knave-shell".into(), "overview".into()])?;
        self.overview_launch_pid = self.children.last().map(|(_, child)| child.id());
        self.overview_cancel_pending = false;
        Ok(())
    }

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
        let overview_became_mapped =
            mapped && !was_mapped && entry.layer.namespace() == "knave-shell-overview";
        if !mapped && entry.layer.namespace() == "knave-shell-overview" {
            self.overview_panes.clear();
        }
        if overview_became_mapped {
            // A modal overview must take pointer input even if an app held a grab.
            self.release_pointer_buttons();
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
        if upper {
            for entry in
                self.shell_surfaces.iter().rev().filter(|entry| {
                    entry.mapped && entry.layer.namespace() == "knave-shell-overview"
                })
            {
                let map = layer_map_for_output(&entry.output);
                let Some(geometry) = map.layer_geometry(&entry.layer) else {
                    continue;
                };
                if geometry.to_f64().contains(self.pointer_location) {
                    let root = entry.layer.wl_surface().clone();
                    let origin = geometry.loc.to_f64();
                    let (surface, offset) = entry
                        .layer
                        .surface_under(self.pointer_location - origin, WindowSurfaceType::ALL)
                        .unwrap_or_else(|| (root.clone(), (0, 0).into()));
                    return Some((
                        surface,
                        origin + offset.to_f64(),
                        Some(KeyboardFocus::Wayland(root)),
                    ));
                }
            }
        }
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

#[cfg(test)]
mod overview_point_tests {
    use super::*;

    #[test]
    fn pane_point_uses_centered_fit_and_rejects_outside() {
        let pane = OverviewPane {
            workspace: WorkspaceId(2),
            x: 100,
            y: 50,
            width: 400,
            height: 300,
        };
        let output = Size::from((800, 400));
        assert_eq!(
            pane_source_point(&pane, output, 300, 200),
            Some((400.0, 200.0))
        );
        assert_eq!(pane_source_point(&pane, output, 99, 200), None);
        assert_eq!(pane_source_point(&pane, output, 500, 200), None);
        // The top letterbox is empty workspace space.
        assert!(pane_source_point(&pane, output, 300, 50).unwrap().1 < 0.0);
    }
}
