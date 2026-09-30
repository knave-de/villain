use super::*;
use smithay::backend::{
    egl::{EGLContext, EGLDisplay, native::EGLSurfacelessDisplay},
    renderer::ExportMem,
};

fn pixels(renderer: &mut GlesRenderer, texture: &mut GlesTexture) -> Vec<u8> {
    let size = texture.size();
    let framebuffer = renderer.bind(texture).unwrap();
    let mapping = renderer
        .copy_framebuffer(&framebuffer, Rectangle::from_size(size), Fourcc::Abgr8888)
        .unwrap();
    renderer.map_texture(&mapping).unwrap().to_vec()
}
fn sample(pixels: &[u8], x: usize, y: usize, width: usize) -> [u8; 4] {
    pixels[(y * width + x) * 4..(y * width + x + 1) * 4]
        .try_into()
        .unwrap()
}
#[test]
fn adjacent_radii_fit_small_windows_proportionally() {
    let r = normalized_radii(
        Corners {
            top_left: 80,
            top_right: 40,
            bottom_right: 10,
            bottom_left: 20,
        },
        (60, 50).into(),
    );
    assert_eq!(r, [40.0, 20.0, 5.0, 10.0]);
}
#[test]
#[ignore = "requires EGL_MESA_platform_surfaceless and a GLES renderer"]
fn gpu_pixels_prove_borders_rounding_opacity_directional_shadows_and_blur() {
    // Surfaceless EGL owns this display and context for the duration of the test.
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay).unwrap() };
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context).unwrap() };
    let mut effects = EffectsRenderer::default();
    effects.ensure_programs(&mut renderer).unwrap();
    let (shape_program, texture_program) = effects.programs.as_ref().unwrap();
    let rect = Rectangle::from_size((64, 64).into());
    let colors = knave_config::appearance::EdgeColors {
        top: "#ff0000ff".into(),
        right: "#00ff00ff".into(),
        bottom: "#0000ffff".into(),
        left: "#ffff00ff".into(),
    };
    let border = border(
        shape_program,
        rect,
        Edges {
            top: 4,
            right: 6,
            bottom: 8,
            left: 10,
        },
        [12.0, 4.0, 16.0, 0.0],
        &colors,
    );
    let mut texture = render_texture(&mut renderer, rect.size, &[border], [0.0; 4]).unwrap();
    let data = pixels(&mut renderer, &mut texture);
    assert_eq!(
        sample(&data, 32, 32, 64),
        [0, 0, 0, 0],
        "border must not cover client content"
    );
    // Smithay exports the offscreen pixels in logical top-to-bottom order.
    assert_eq!(sample(&data, 32, 1, 64), [255, 0, 0, 255]);
    assert_eq!(sample(&data, 62, 32, 64), [0, 255, 0, 255]);
    assert_eq!(sample(&data, 32, 62, 64), [0, 0, 255, 255]);
    assert_eq!(sample(&data, 1, 32, 64), [255, 255, 0, 255]);
    assert_eq!(
        sample(&data, 0, 0, 64)[3],
        0,
        "rounded corner must be clear"
    );
    let source =
        render_texture::<EffectElement>(&mut renderer, rect.size, &[], [1.0, 0.0, 0.0, 1.0])
            .unwrap();
    let rounded = filtered(
        &renderer,
        source,
        texture_program,
        (0, 0).into(),
        [16.0, 0.0, 0.0, 0.0],
        0.5,
        (0.0, 0.0),
        0.0,
    );
    let mut texture = render_texture(&mut renderer, rect.size, &[rounded], [0.0; 4]).unwrap();
    let data = pixels(&mut renderer, &mut texture);
    assert!((127..=129).contains(&sample(&data, 32, 32, 64)[3]));
    assert_eq!(sample(&data, 0, 0, 64)[3], 0);
    assert!((127..=129).contains(&sample(&data, 63, 0, 64)[3]));
    let shadow = knave_config::appearance::Shadow {
        enabled: true,
        color: "#ff0000ff".into(),
        blur_radius: 4,
        ..Default::default()
    };
    let body = Rectangle::new((16, 16).into(), (32, 32).into());
    let shadow = shadow_element(shape_program, body, [0.0; 4], 1, &shadow);
    let mut texture = render_texture(&mut renderer, rect.size, &[shadow], [0.0; 4]).unwrap();
    let data = pixels(&mut renderer, &mut texture);
    assert!(
        sample(&data, 49, 32, 64)[3] > 100,
        "right shadow must be visible"
    );
    assert_eq!(
        sample(&data, 14, 32, 64)[3],
        0,
        "right shadow cannot paint left edge"
    );
    assert_eq!(
        sample(&data, 32, 32, 64)[3],
        0,
        "shadow cannot cover client content"
    );
    let solid = smithay::backend::renderer::element::solid::SolidColorBuffer::new(
        (32, 64),
        [1.0, 1.0, 1.0, 1.0],
    );
    let element = smithay::backend::renderer::element::solid::SolidColorRenderElement::from_buffer(
        &solid,
        (0, 0),
        1.0,
        1.0,
        Kind::Unspecified,
    );
    let source =
        render_texture(&mut renderer, rect.size, &[element], [0.0, 0.0, 0.0, 1.0]).unwrap();
    let blurred = filtered(
        &renderer,
        source,
        texture_program,
        (0, 0).into(),
        [0.0; 4],
        1.0,
        (1.0, 0.0),
        12.0,
    );
    let mut texture = render_texture(&mut renderer, rect.size, &[blurred], [0.0; 4]).unwrap();
    let data = pixels(&mut renderer, &mut texture);
    let near_edge = sample(&data, 34, 32, 64)[0];
    assert!(
        near_edge > 0 && near_edge < 255,
        "blur must mix actual backdrop pixels: {near_edge}"
    );
    assert_eq!(
        sample(&data, 60, 32, 64)[0],
        0,
        "blur must not spread without bound"
    );
}
