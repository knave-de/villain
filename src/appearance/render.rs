//! Event-driven effects composition. One bounded LRU owns finished scene textures.
pub(crate) use paint::normalized_radii;
#[path = "paint.rs"]
mod paint;
use crate::{focus::KeyboardFocus, state::Villain};
use knave_config::{
    WindowAppearance,
    appearance::{Corners, Edges, ViewEffects, rgba},
};
use paint::*;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Offscreen, Texture,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Element, Id, Kind,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                texture::{TextureBuffer, TextureRenderElement},
                utils::{Relocate, RelocateRenderElement},
            },
            gles::{
                GlesError, GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture, Uniform,
                UniformName, UniformType,
                element::{PixelShaderElement, TextureShaderElement},
            },
            utils::CommitCounter,
        },
    },
    desktop::{PopupManager, layer_map_for_output},
    utils::{Logical, Physical, Rectangle, Scale, Size, Transform},
    wayland::{seat::WaylandFocus, shell::wlr_layer::Layer},
};
use std::collections::VecDeque;

smithay::backend::renderer::element::render_elements! {
    pub EffectElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Shape=PixelShaderElement,
    Texture=TextureRenderElement<GlesTexture>,
    Filter=TextureShaderElement,
}

#[derive(Debug)]
pub(crate) enum EffectError {
    TargetTooLarge,
    Renderer(GlesError),
    Damage(smithay::backend::renderer::damage::Error<GlesError>),
    Interrupted(smithay::backend::renderer::sync::Interrupted),
    OutsideOutput,
}
impl std::fmt::Display for EffectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetTooLarge => f.write_str("effect target exceeds 36 Mi pixels"),
            Self::Renderer(error) => write!(f, "effect renderer failed: {error}"),
            Self::Damage(error) => write!(f, "effect composition failed: {error}"),
            Self::Interrupted(error) => error.fmt(f),
            Self::OutsideOutput => f.write_str("blur rectangle outside output"),
        }
    }
}
impl std::error::Error for EffectError {}

type Geometry = Rectangle<i32, Logical>;
type SourceKey = (
    Id,
    CommitCounter,
    Rectangle<i32, Physical>,
    u32,
    Rectangle<f64, smithay::utils::Buffer>,
    Transform,
);
#[derive(PartialEq)]
struct SceneKey {
    workspace: usize,
    layers: bool,
    size: Size<i32, Logical>,
    appearance: WindowAppearance,
    sources: Vec<SourceKey>,
    windows: Vec<(Geometry, bool, bool, bool, bool)>,
}
struct CachedScene {
    key: SceneKey,
    texture: TextureBuffer<GlesTexture>,
}
#[derive(Default)]
pub(crate) struct EffectsRenderer {
    programs: Option<(GlesPixelProgram, GlesTexProgram)>,
    scenes: VecDeque<CachedScene>,
}
const MAX_PIXELS: i64 = 36 * 1024 * 1024;

impl EffectsRenderer {
    pub(crate) fn clear(&mut self) {
        self.scenes.clear();
    }
    pub(crate) fn scene(
        &mut self,
        state: &Villain,
        renderer: &mut GlesRenderer,
        workspace: usize,
        layers: bool,
    ) -> Result<Vec<EffectElement>, EffectError> {
        let size = state.output_size;
        let focused = state.keyboard.current_focus();
        let windows: Vec<_> = state
            .workspace_layout(workspace)
            .into_iter()
            .filter(|entry| entry.5)
            .collect();
        let mut key = SceneKey {
            workspace,
            layers,
            size,
            appearance: state.config.appearance.clone(),
            sources: Vec::new(),
            windows: Vec::new(),
        };
        let mut lower = Vec::<EffectElement>::new();
        let mut upper = Vec::<EffectElement>::new();
        if layers {
            for output in state.space.outputs() {
                let map = layer_map_for_output(output);
                for layer in map.layers() {
                    let Some(geometry) = map.layer_geometry(layer) else {
                        continue;
                    };
                    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = layer
                        .render_elements(
                            renderer,
                            geometry.loc.to_physical_precise_round(1.0),
                            Scale::from(1.0),
                            1.0,
                        );
                    let target = if matches!(layer.layer(), Layer::Top | Layer::Overlay) {
                        &mut upper
                    } else {
                        &mut lower
                    };
                    key.sources.extend(elements.iter().map(source_key));
                    target.splice(0..0, elements.into_iter().map(EffectElement::Surface));
                }
            }
        }
        let mut sources = Vec::new();
        for (window, rect, fullscreen, maximized, tiled, _) in &windows {
            let is_focused = KeyboardFocus::for_window(window) == focused;
            key.windows
                .push((*rect, *fullscreen, *maximized, *tiled, is_focused));
            let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = window.render_elements(
                renderer,
                (rect.loc - window.geometry().loc).to_physical_precise_round(1.0),
                Scale::from(1.0),
                1.0,
            );
            key.sources.extend(elements.iter().map(source_key));
            sources.push(elements);
        }
        // Include override-redirect X11 surfaces without adding decorations.
        let mut unmanaged = Vec::new();
        if layers {
            for window in state.space.elements() {
                if windows.iter().any(|entry| entry.0 == *window) {
                    continue;
                }
                let Some(loc) = state.space.element_location(window) else {
                    continue;
                };
                let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = window
                    .render_elements(
                        renderer,
                        (loc - window.geometry().loc).to_physical_precise_round(1.0),
                        Scale::from(1.0),
                        1.0,
                    );
                key.sources.extend(elements.iter().map(source_key));
                unmanaged.splice(0..0, elements.into_iter().map(EffectElement::Surface));
            }
        }
        let any_effect = windows
            .iter()
            .any(|(_, _, fullscreen, maximized, tiled, _)| {
                let a = &state.config.appearance;
                let view = if *fullscreen {
                    ViewEffects::NONE
                } else if *maximized {
                    a.views.maximized
                } else if *tiled {
                    a.views.tiled
                } else {
                    a.views.floating
                };
                (view.borders && a.border.width != Edges::default())
                    || (view.radius && a.border.radius != Corners::default())
                    || [&a.focused, &a.unfocused].into_iter().any(|style| {
                        (view.opacity && style.opacity != 100)
                            || (view.blur && style.blur.enabled && style.blur.radius > 0)
                            || (view.shadows
                                && [
                                    &style.shadows.top,
                                    &style.shadows.right,
                                    &style.shadows.bottom,
                                    &style.shadows.left,
                                ]
                                .iter()
                                .any(|s| s.enabled))
                    })
            });
        if !any_effect {
            self.scenes
                .retain(|entry| entry.key.workspace != workspace || entry.key.layers != layers);
            for source in sources {
                lower.splice(0..0, source.into_iter().map(EffectElement::Surface));
            }
            unmanaged.extend(lower);
            upper.extend(unmanaged);
            return Ok(upper);
        }
        if size.w <= 0 || size.h <= 0 || i64::from(size.w) * i64::from(size.h) > MAX_PIXELS {
            return Err(EffectError::TargetTooLarge);
        }
        if let Some(index) = self.scenes.iter().position(|entry| entry.key == key) {
            let entry = self.scenes.remove(index).expect("cache index exists");
            let element = texture_element(&entry.texture, (0, 0), 1.0);
            self.scenes.push_back(entry);
            return Ok(vec![EffectElement::Texture(element)]);
        }
        self.ensure_programs(renderer)
            .map_err(EffectError::Renderer)?;
        let (shape_program, texture_program) = self.programs.as_ref().expect("programs compiled");
        let appearance = &state.config.appearance;
        let background = if layers {
            [0.08, 0.05, 0.12, 1.0]
        } else {
            [0.035, 0.047, 0.063, 1.0]
        };
        let mut elements = lower;
        let mut live_pixels = 0_i64;
        for ((window, rect, fullscreen, maximized, tiled, _), original) in
            windows.into_iter().zip(sources)
        {
            let view = if fullscreen {
                ViewEffects::NONE
            } else if maximized {
                appearance.views.maximized
            } else if tiled {
                appearance.views.tiled
            } else {
                appearance.views.floating
            };
            let style = if KeyboardFocus::for_window(&window) == focused {
                &appearance.focused
            } else {
                &appearance.unfocused
            };
            let opacity = if view.opacity {
                f32::from(style.opacity) / 100.0
            } else {
                1.0
            };
            let widths = if view.borders {
                appearance.border.width
            } else {
                Edges::default()
            };
            let outer = expand(rect, widths);
            let radii = normalized_radii(
                if view.radius {
                    appearance.border.radius
                } else {
                    Corners::default()
                },
                outer.size,
            );
            let inner_radii = [
                (radii[0] - f32::from(widths.left.max(widths.top))).max(0.0),
                (radii[1] - f32::from(widths.right.max(widths.top))).max(0.0),
                (radii[2] - f32::from(widths.right.max(widths.bottom))).max(0.0),
                (radii[3] - f32::from(widths.left.max(widths.bottom))).max(0.0),
            ];
            // Flatten retained lower content before another effect window can exceed
            // the scene pixel budget. Transient targets remain bounded independently.
            let window_pixels = i64::from(rect.size.w) * i64::from(rect.size.h) * 2;
            if live_pixels + window_pixels > MAX_PIXELS && live_pixels > 0 {
                let lower_texture = render_texture(renderer, size, &elements, background)?;
                let lower_buffer = TextureBuffer::from_texture(
                    renderer,
                    lower_texture,
                    1,
                    Transform::Normal,
                    None,
                );
                elements = vec![EffectElement::Texture(texture_element(
                    &lower_buffer,
                    (0, 0),
                    1.0,
                ))];
                live_pixels = i64::from(size.w) * i64::from(size.h);
            }
            live_pixels += window_pixels;
            let mut window_elements = Vec::new();
            // Root/subsurfaces are clipped as a group. Popups remain outside the window mask.
            if radii.iter().any(|r| *r > 0.0) || opacity != 1.0 {
                if let Some(root) = window.wl_surface() {
                    let root_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                        render_elements_from_surface_tree(
                            renderer,
                            &root,
                            smithay::utils::Point::<i32, Logical>::from((
                                -window.geometry().loc.x,
                                -window.geometry().loc.y,
                            ))
                            .to_physical_precise_round(1.0),
                            Scale::from(1.0),
                            1.0,
                            Kind::Unspecified,
                        );
                    let content = render_texture(renderer, rect.size, &root_elements, [0.0; 4])?;
                    window_elements.push(filtered(
                        renderer,
                        content,
                        texture_program,
                        rect.loc,
                        inner_radii,
                        opacity,
                        (0.0, 0.0),
                        0.0,
                    ));
                    for (popup, location) in PopupManager::popups_for_surface(&root) {
                        let location = rect.loc + location - popup.geometry().loc;
                        let popup_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                            render_elements_from_surface_tree(
                                renderer,
                                popup.wl_surface(),
                                location.to_physical_precise_round(1.0),
                                Scale::from(1.0),
                                opacity,
                                Kind::Unspecified,
                            );
                        window_elements
                            .splice(0..0, popup_elements.into_iter().map(EffectElement::Surface));
                    }
                }
            } else {
                window_elements.extend(original.into_iter().map(EffectElement::Surface));
            }
            if widths != Edges::default() {
                window_elements.push(EffectElement::Shape(border(
                    shape_program,
                    outer,
                    widths,
                    radii,
                    &style.border_color,
                )));
            }
            if view.blur && style.blur.enabled && style.blur.radius > 0 {
                // Capture only lower stacking content; cursor, this window and upper layers are excluded.
                let padding = i32::from(style.blur.radius) * i32::from(style.blur.passes);
                let capture = Rectangle::new(
                    (rect.loc.x - padding, rect.loc.y - padding).into(),
                    (rect.size.w + padding * 2, rect.size.h + padding * 2).into(),
                )
                .intersection(Rectangle::from_size(size))
                .ok_or(EffectError::OutsideOutput)?;
                let relocated: Vec<_> = elements
                    .iter()
                    .map(|element| {
                        RelocateRenderElement::from_element(
                            element,
                            (-capture.loc.x, -capture.loc.y),
                            Relocate::Relative,
                        )
                    })
                    .collect();
                let mut backdrop = render_texture(renderer, capture.size, &relocated, background)?;
                for _ in 0..style.blur.passes {
                    for direction in [(1.0, 0.0), (0.0, 1.0)] {
                        let element = filtered(
                            renderer,
                            backdrop,
                            texture_program,
                            (0, 0).into(),
                            [0.0; 4],
                            1.0,
                            direction,
                            f32::from(style.blur.radius),
                        );
                        backdrop = render_texture(renderer, capture.size, &[element], [0.0; 4])?;
                    }
                }
                let source_rect = Rectangle::new(rect.loc - capture.loc, rect.size);
                window_elements.push(filtered_region(
                    renderer,
                    backdrop,
                    texture_program,
                    rect,
                    source_rect,
                    inner_radii,
                ));
            }
            if view.shadows {
                for (side, shadow) in [
                    &style.shadows.top,
                    &style.shadows.right,
                    &style.shadows.bottom,
                    &style.shadows.left,
                ]
                .into_iter()
                .enumerate()
                {
                    if shadow.enabled {
                        window_elements.push(EffectElement::Shape(shadow_element(
                            shape_program,
                            outer,
                            radii,
                            side,
                            shadow,
                        )));
                    }
                }
            }
            window_elements.extend(elements);
            elements = window_elements;
        }
        unmanaged.extend(elements);
        upper.extend(unmanaged);
        let texture = render_texture(renderer, size, &upper, background)?;
        let texture = TextureBuffer::from_texture(
            renderer,
            texture,
            1,
            Transform::Normal,
            Some(vec![Rectangle::from_size((size.w, size.h).into())]),
        );
        let element = texture_element(&texture, (0, 0), 1.0);
        // Replace stale scene for the same workspace; LRU eviction bounds resident pixels.
        self.scenes
            .retain(|entry| entry.key.workspace != workspace || entry.key.layers != layers);
        let pixels =
            |entry: &CachedScene| i64::from(entry.key.size.w) * i64::from(entry.key.size.h);
        while self.scenes.len() >= 10
            || self.scenes.iter().map(pixels).sum::<i64>() + i64::from(size.w) * i64::from(size.h)
                > MAX_PIXELS
        {
            self.scenes.pop_front();
        }
        self.scenes.push_back(CachedScene { key, texture });
        Ok(vec![EffectElement::Texture(element)])
    }
    fn ensure_programs(&mut self, renderer: &mut GlesRenderer) -> Result<(), GlesError> {
        if self.programs.is_none() {
            let shape = renderer.compile_custom_pixel_shader(
                include_str!("shape.glsl"),
                &[
                    UniformName::new("radii", UniformType::_4f),
                    UniformName::new("widths", UniformType::_4f),
                    UniformName::new("top_color", UniformType::_4f),
                    UniformName::new("right_color", UniformType::_4f),
                    UniformName::new("bottom_color", UniformType::_4f),
                    UniformName::new("left_color", UniformType::_4f),
                    UniformName::new("body", UniformType::_4f),
                    UniformName::new("shadow_data", UniformType::_4f),
                    UniformName::new("shadow_color", UniformType::_4f),
                    UniformName::new("side", UniformType::_1f),
                ],
            )?;
            let texture = renderer.compile_custom_texture_shader(
                include_str!("texture.glsl"),
                &[
                    UniformName::new("extent", UniformType::_2f),
                    UniformName::new("mask_rect", UniformType::_4f),
                    UniformName::new("radii", UniformType::_4f),
                    UniformName::new("direction", UniformType::_2f),
                    UniformName::new("blur_radius", UniformType::_1f),
                    UniformName::new("flip_y", UniformType::_1f),
                ],
            )?;
            self.programs = Some((shape, texture));
        }
        Ok(())
    }
}
#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
