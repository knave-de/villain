//! Bounded, on-demand workspace previews for trusted shell clients.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, Offscreen,
            damage::OutputDamageTracker,
            element::{AsRenderElements, surface::WaylandSurfaceRenderElement},
            gles::{GlesRenderbuffer, GlesRenderer},
        },
    },
    utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::{state::Villain, workspaces::WorkspacePreviewScene};

const MIN_WIDTH: u32 = 64;
const MIN_HEIGHT: u32 = 36;
const MAX_WIDTH: u32 = 16384;
const MAX_HEIGHT: u32 = 16384;
const MAX_PIXELS: u64 = 36 * 1024 * 1024;

pub fn capture(
    state: &mut Villain,
    workspace: usize,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    if !(MIN_WIDTH..=MAX_WIDTH).contains(&width)
        || !(MIN_HEIGHT..=MAX_HEIGHT).contains(&height)
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(format!(
            "preview dimensions must be within {MIN_WIDTH}x{MIN_HEIGHT} and {MAX_WIDTH}x{MAX_HEIGHT}, with at most {MAX_PIXELS} pixels"
        ));
    }
    let scene = workspace
        .checked_sub(1)
        .and_then(|index| state.workspace_preview_scene(index))
        .ok_or_else(|| format!("workspace {workspace} does not exist"))?;

    if let Some(tty) = state.tty.as_mut() {
        render(&mut tty.renderer, scene, width, height)
    } else if let Some(winit) = state.winit.as_mut() {
        render(winit.renderer(), scene, width, height)
    } else {
        Err("no renderer is available".into())
    }
}

fn render(
    renderer: &mut GlesRenderer,
    scene: WorkspacePreviewScene,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let buffer_size: Size<i32, Buffer> = (width as i32, height as i32).into();
    let physical_size: Size<i32, Physical> = (width as i32, height as i32).into();
    let source_width = scene.output_size.w.max(1) as f64;
    let source_height = scene.output_size.h.max(1) as f64;
    let scale = (f64::from(width) / source_width).min(f64::from(height) / source_height);
    let content_width = (source_width * scale).round() as i32;
    let content_height = (source_height * scale).round() as i32;
    let offset: Point<i32, Physical> = (
        (width as i32 - content_width) / 2,
        (height as i32 - content_height) / 2,
    )
        .into();

    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = scene
        .windows
        .into_iter()
        .rev()
        .flat_map(|(window, geometry)| {
            let location = offset + geometry.loc.to_physical_precise_round(Scale::from(scale));
            AsRenderElements::<GlesRenderer>::render_elements(
                &window,
                renderer,
                location,
                Scale::from(scale),
                1.0,
            )
        })
        .collect();

    let mut target =
        Offscreen::<GlesRenderbuffer>::create_buffer(renderer, Fourcc::Abgr8888, buffer_size)
            .map_err(|error| format!("could not allocate preview target: {error}"))?;
    let mut framebuffer = renderer
        .bind(&mut target)
        .map_err(|error| format!("could not bind preview target: {error}"))?;
    // Surface sizes are resolved using the damage tracker scale, independently
    // of the physical positions computed above. Both must use thumbnail scale.
    let mut damage = OutputDamageTracker::new(physical_size, scale, Transform::Normal);
    let result = damage
        .render_output(
            renderer,
            &mut framebuffer,
            0,
            &elements,
            [0.035, 0.047, 0.063, 1.0],
        )
        .map_err(|error| format!("could not render preview: {error:?}"))?;
    result
        .sync
        .wait()
        .map_err(|error| format!("preview synchronization failed: {error}"))?;

    let mapping = renderer
        .copy_framebuffer(
            &framebuffer,
            Rectangle::from_size(buffer_size),
            Fourcc::Abgr8888,
        )
        .map_err(|error| format!("could not read preview: {error}"))?;
    let pixels = renderer
        .map_texture(&mapping)
        .map_err(|error| format!("could not map preview: {error}"))?
        .to_vec();
    encode_png(&pixels, width, height)
}

fn encode_png(pixels: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("could not encode preview header: {error}"))?;
        writer
            .write_image_data(pixels)
            .map_err(|error| format!("could not encode preview pixels: {error}"))?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_encoder_produces_png_signature() {
        let png = encode_png(&[0; 4 * 4 * 4], 4, 4).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}
