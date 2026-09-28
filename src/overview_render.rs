//! Render-only workspace surfaces inside shell-owned overview panes.

use smithay::{
    backend::renderer::{
        element::{
            AsRenderElements,
            surface::WaylandSurfaceRenderElement,
            utils::{
                ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, Relocate,
                RelocateRenderElement, RescaleRenderElement, constrain_render_elements,
            },
        },
        gles::GlesRenderer,
    },
    utils::{Physical, Rectangle, Scale},
};

use crate::{cursor::CursorRenderElement, state::Villain};

type PaneElement = RelocateRenderElement<
    CropRenderElement<
        RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
    >,
>;

smithay::backend::renderer::element::render_elements! {
    pub OverviewRenderElement<=GlesRenderer>;
    Cursor=CursorRenderElement,
    Pane=PaneElement,
}

pub fn elements(
    state: &Villain,
    renderer: &mut GlesRenderer,
    cursor: Vec<CursorRenderElement>,
) -> Vec<OverviewRenderElement> {
    let mut result: Vec<_> = cursor
        .into_iter()
        .map(OverviewRenderElement::Cursor)
        .collect();
    let output = state.output_size;
    let reference: Rectangle<i32, Physical> = Rectangle::from_size((output.w, output.h).into());
    for pane in state.overview_panes.iter().rev() {
        let Some(scene) = state.workspace_preview_scene(pane.workspace.0 as usize - 1) else {
            continue;
        };
        let Ok(width) = i32::try_from(pane.width) else {
            continue;
        };
        let Ok(height) = i32::try_from(pane.height) else {
            continue;
        };
        // Smithay constrains in the source coordinate space; the crop must be
        // pane-local, then the completed element moves to its output position.
        let crop = Rectangle::from_size((width, height).into());
        let windows: Vec<_> = scene
            .windows
            .into_iter()
            .rev()
            .flat_map(|(window, geometry)| {
                AsRenderElements::<GlesRenderer>::render_elements(
                    &window,
                    renderer,
                    geometry.loc.to_physical_precise_round(Scale::from(1.0)),
                    Scale::from(1.0),
                    1.0,
                )
            })
            .collect();
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
    result
}
