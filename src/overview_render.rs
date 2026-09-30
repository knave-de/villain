//! Render-only workspace surfaces inside shell-owned overview panes.

use smithay::{
    backend::renderer::{
        element::utils::{
            ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, Relocate,
            RelocateRenderElement, RescaleRenderElement, constrain_render_elements,
        },
        gles::GlesRenderer,
    },
    utils::{Physical, Rectangle, Scale},
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
    Scene=crate::appearance::render::EffectElement,
}

pub fn elements(
    state: &mut Villain,
    renderer: &mut GlesRenderer,
    cursor: Vec<CursorRenderElement>,
) -> Result<Vec<OverviewRenderElement>, crate::appearance::render::EffectError> {
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
