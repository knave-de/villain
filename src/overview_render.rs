//! Render-only workspace surfaces inside shell-owned overview panes.

use smithay::{
    backend::renderer::{
        element::utils::{
            ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, Relocate,
            RelocateRenderElement, RescaleRenderElement, constrain_render_elements,
        },
        element::{
            Id, Kind, memory::MemoryRenderBufferRenderElement, solid::SolidColorRenderElement,
        },
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    utils::{Physical, Rectangle, Scale, Size},
};

use crate::{cursor::CursorRenderElement, state::Villain};

type PaneElement = RelocateRenderElement<
    CropRenderElement<
        RelocateRenderElement<RescaleRenderElement<crate::appearance::render::EffectElement>>,
    >,
>;

smithay::backend::renderer::element::render_elements! {
    pub OverviewRenderElement<=GlesRenderer>;
    Cursor=CursorRenderElement,
    Pane=PaneElement,
    Window=crate::capture_window::WindowElement,
    Scene=crate::appearance::render::EffectElement,
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
    let output = state.output_size;
    let reference: Rectangle<i32, Physical> = Rectangle::from_size((output.w, output.h).into());
    for pane in state.overview_panes.clone().iter().rev() {
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
        // Smithay constrains in the source coordinate space; the crop must be
        // pane-local, then the completed element moves to its output position.
        let crop = Rectangle::from_size((width, height).into());
        let mut effects = std::mem::take(&mut state.appearance_renderer);
        let windows = effects.scene(state, renderer, pane.workspace.0 as usize - 1, false);
        state.appearance_renderer = effects;
        let windows = windows?;
        result.extend(
            constrain_render_elements(
                windows,
                (0, 0),
                crop,
                reference,
                ConstrainScaleBehavior::Fit,
                ConstrainAlign::CENTER,
                Scale::from(1.0),
            )
            .map(|element| {
                RelocateRenderElement::from_element(element, (pane.x, pane.y), Relocate::Relative)
            })
            .map(OverviewRenderElement::Pane),
        );
    }
    let mut effects = std::mem::take(&mut state.appearance_renderer);
    let scene = effects.scene(state, renderer, state.active_workspace, true);
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
