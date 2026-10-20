//! Render-only workspace surfaces inside shell-owned overview panes.

use smithay::{
    backend::renderer::{
        element::{
            Id, Kind,
            memory::MemoryRenderBufferRenderElement,
            solid::SolidColorRenderElement,
            utils::{CropRenderElement, Relocate, RelocateRenderElement},
        },
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    utils::{Physical, Rectangle, Scale, Size},
};

use crate::{cursor::CursorRenderElement, state::Villain};

smithay::backend::renderer::element::render_elements! {
    pub OverviewRenderElement<=GlesRenderer>;
    Cursor=CursorRenderElement,
    Overlay=smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
    Window=crate::capture_window::WindowElement,
    Scene=crate::appearance::render::EffectElement,
    Swipe=CropRenderElement<RelocateRenderElement<crate::appearance::render::EffectElement>>,
    Screenshot=MemoryRenderBufferRenderElement<GlesRenderer>,
    ScreenshotOverlay=SolidColorRenderElement,
}

pub fn elements(
    state: &mut Villain,
    renderer: &mut GlesRenderer,
    cursor: Vec<CursorRenderElement>,
) -> Result<Vec<OverviewRenderElement>, crate::appearance::render::EffectError> {
    if let Some(background) = state.screenshot.background() {
        return screenshot_elements(state, renderer, cursor, background);
    }
    let mut result: Vec<_> = cursor
        .into_iter()
        .map(OverviewRenderElement::Cursor)
        .collect();
    let cursor_len = result.len();
    let now = std::time::Instant::now();
    let scenes = if let Some(transition) = state.gestures.transition {
        let incoming = transition.offset
            + f64::from(transition.direction.incoming_offset(state.output_size.w));
        vec![
            (transition.source, transition.offset),
            (transition.target, incoming),
        ]
    } else {
        state.workspace_motion.scenes(now)
    };
    if !scenes.is_empty() {
        use smithay::{
            backend::renderer::element::AsRenderElements, desktop::layer_map_for_output,
        };
        // Keep shell UI stationary, above the two clipped workspace scenes.
        for output in state.space.outputs() {
            let map = layer_map_for_output(output);
            for layer in map
                .layers()
                .filter(|layer| layer.namespace() != "knave-shell-wallpaper")
            {
                if !state.layer_surface_visible(layer.wl_surface()) {
                    continue;
                }
                if let Some(geometry) = map.layer_geometry(layer) {
                    let elements: Vec<
                        smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<
                            GlesRenderer,
                        >,
                    > = layer.render_elements(
                        renderer,
                        geometry.loc.to_physical_precise_round(1.0),
                        Scale::from(1.0),
                        1.0,
                    );
                    result.splice(
                        cursor_len..cursor_len,
                        elements.into_iter().map(OverviewRenderElement::Overlay),
                    );
                }
            }
        }
        let width = state.output_size.w;
        let bounds = Rectangle::from_size((width, state.output_size.h).into());
        let mut effects = std::mem::take(&mut state.appearance_renderer);
        let scenes = (|| {
            for (workspace, x) in scenes {
                let x = x.round() as i32;
                let clip = Rectangle::new((x, 0).into(), (width, state.output_size.h).into());
                let Some(clip) = clip.intersection(bounds) else {
                    continue;
                };
                let scene = effects.scene(
                    state,
                    renderer,
                    workspace,
                    crate::appearance::render::SceneLayers::Wallpaper,
                )?;
                result.extend(scene.into_iter().filter_map(|element| {
                    let shifted =
                        RelocateRenderElement::from_element(element, (x, 0), Relocate::Relative);
                    CropRenderElement::from_element(shifted, 1.0, clip)
                        .map(OverviewRenderElement::Swipe)
                }));
            }
            Ok(result)
        })();
        state.appearance_renderer = effects;
        return scenes;
    }
    let output = state.output_size;
    let overview_mapped = state.overview_mapped();
    if overview_mapped {
        use smithay::{
            backend::renderer::element::AsRenderElements, desktop::layer_map_for_output,
        };
        for output in state.space.outputs() {
            let map = layer_map_for_output(output);
            for layer in map
                .layers()
                .filter(|layer| layer.namespace() == "knave-shell-overview")
            {
                if !state.layer_surface_visible(layer.wl_surface()) {
                    continue;
                }
                if let Some(geometry) = map.layer_geometry(layer) {
                    let elements: Vec<
                        smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<
                            GlesRenderer,
                        >,
                    > = layer.render_elements(
                        renderer,
                        geometry.loc.to_physical_precise_round(1.0),
                        Scale::from(1.0),
                        1.0,
                    );
                    result.extend(elements.into_iter().map(OverviewRenderElement::Overlay));
                }
            }
        }
    }
    for pane in state
        .overview_panes
        .clone()
        .iter()
        .rev()
        .filter(|_| overview_mapped)
    {
        if state
            .workspace_preview_scene(pane.workspace.0 as usize - 1)
            .is_none()
        {
            continue;
        }
        let Ok(width) = i32::try_from(pane.width) else {
            continue;
        };
        let Ok(height) = i32::try_from(pane.height) else {
            continue;
        };
        let mut effects = std::mem::take(&mut state.appearance_renderer);
        let element = effects.overview_pane(
            state,
            renderer,
            pane.workspace.0 as usize - 1,
            Rectangle::new((pane.x, pane.y).into(), (width, height).into()),
        );
        state.appearance_renderer = effects;
        result.push(OverviewRenderElement::Scene(element?));
    }
    if overview_mapped {
        result.push(OverviewRenderElement::ScreenshotOverlay(
            SolidColorRenderElement::new(
                state.overview_backdrop_id.clone(),
                Rectangle::from_size((output.w, output.h).into()),
                CommitCounter::default(),
                [0.0, 0.0, 0.0, 0.5],
                Kind::Unspecified,
            ),
        ));
    }
    let mut effects = std::mem::take(&mut state.appearance_renderer);
    let scene = effects.scene(
        state,
        renderer,
        state.active_workspace,
        crate::appearance::render::SceneLayers::All,
    );
    state.appearance_renderer = effects;
    result.extend(scene?.into_iter().map(OverviewRenderElement::Scene));
    Ok(result)
}

fn screenshot_elements(
    state: &Villain,
    renderer: &mut GlesRenderer,
    cursor: Vec<CursorRenderElement>,
    background: &smithay::backend::renderer::element::memory::MemoryRenderBuffer,
) -> Result<Vec<OverviewRenderElement>, crate::appearance::render::EffectError> {
    // Render elements are front-to-back; keep the frozen image behind the tint and cursor.
    let mut result: Vec<_> = cursor
        .into_iter()
        .map(OverviewRenderElement::Cursor)
        .collect();
    let image = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, 0.0),
        background,
        None,
        None,
        None,
        Kind::Unspecified,
    )
    .map_err(crate::appearance::render::EffectError::Renderer)?;
    let size = state
        .screenshot
        .frame_size()
        .unwrap_or_else(|| Size::from((state.output_size.w, state.output_size.h)));
    let overlay_ids = state
        .screenshot
        .overlay_ids()
        .expect("screenshot selection");
    let full = Rectangle::from_size((size.w, size.h).into());
    if let Some(selection) = state.screenshot.selection_rect() {
        let x0 = selection.loc.x.clamp(0, full.size.w);
        let y0 = selection.loc.y.clamp(0, full.size.h);
        let x1 = selection
            .loc
            .x
            .saturating_add(selection.size.w)
            .clamp(0, full.size.w);
        let y1 = selection
            .loc
            .y
            .saturating_add(selection.size.h)
            .clamp(0, full.size.h);
        let shade = [0.0, 0.0, 0.0, 0.38];
        for (index, rect) in [
            Rectangle::new((0, 0).into(), (full.size.w, y0).into()),
            Rectangle::new((0, y1).into(), (full.size.w, full.size.h - y1).into()),
            Rectangle::new((0, y0).into(), (x0, y1 - y0).into()),
            Rectangle::new((x1, y0).into(), (full.size.w - x1, y1 - y0).into()),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(element) = solid(overlay_ids[index].clone(), rect, shade) {
                result.push(OverviewRenderElement::ScreenshotOverlay(element));
            }
        }
        let border = [0.25, 0.78, 1.0, 1.0];
        let thickness = 2.min(selection.size.w).min(selection.size.h);
        for (index, rect) in [
            Rectangle::new(selection.loc, (selection.size.w, thickness).into()),
            Rectangle::new(
                (
                    selection.loc.x,
                    selection.loc.y + selection.size.h - thickness,
                )
                    .into(),
                (selection.size.w, thickness).into(),
            ),
            Rectangle::new(selection.loc, (thickness, selection.size.h).into()),
            Rectangle::new(
                (
                    selection.loc.x + selection.size.w - thickness,
                    selection.loc.y,
                )
                    .into(),
                (thickness, selection.size.h).into(),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(element) = solid(overlay_ids[index + 4].clone(), rect, border) {
                result.push(OverviewRenderElement::ScreenshotOverlay(element));
            }
        }
    } else {
        if let Some(element) = solid(overlay_ids[0].clone(), full, [0.0, 0.0, 0.0, 0.12]) {
            result.push(OverviewRenderElement::ScreenshotOverlay(element));
        }
    }
    result.push(OverviewRenderElement::Screenshot(image));
    Ok(result)
}

fn solid(
    id: Id,
    rect: Rectangle<i32, Physical>,
    color: [f32; 4],
) -> Option<SolidColorRenderElement> {
    (rect.size.w > 0 && rect.size.h > 0).then(|| {
        SolidColorRenderElement::new(id, rect, CommitCounter::default(), color, Kind::Unspecified)
    })
}
