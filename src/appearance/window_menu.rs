//! Small, cached software raster for the compositor's universal window menu.
use cosmic_text::{Attrs, Buffer, Color as TextColor, FontSystem, Metrics, Shaping, SwashCache};
use knave_ui::toolkit::{Element, ElementId, Scene, Widget};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};
use std::collections::VecDeque;

const ROW_HEIGHT: i32 = 30;
const INSET: i32 = 0;
const MAX_WIDTH: i32 = 512;
const MAX_HEIGHT: i32 = 256;
const CACHE_ENTRIES: usize = 8;

fn bgra(rgba: [u8; 4]) -> [u8; 4] {
    [rgba[2], rgba[1], rgba[0], rgba[3]]
}

fn line(pixels: &mut [u8], width: i32, height: i32, color: [u8; 4], a: (i32, i32), b: (i32, i32)) {
    let (mut x0, mut y0) = a;
    let (x1, y1) = b;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        if (0..width).contains(&x0) && (0..height).contains(&y0) {
            let index = ((y0 * width + x0) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&bgra(color));
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let twice = 2 * error;
        if twice >= dy {
            error += dy;
            x0 += sx;
        }
        if twice <= dx {
            error += dx;
            y0 += sy;
        }
    }
}

fn icon(
    pixels: &mut [u8],
    width: i32,
    height: i32,
    y: i32,
    kind: usize,
    tiled: bool,
    color: [u8; 4],
) {
    let x = 20;
    match kind {
        0 => line(pixels, width, height, color, (x, y + 15), (x + 12, y + 15)),
        1 => {
            if tiled {
                // Tiled windows offer a floating action, represented by a
                // pair of overlapping window outlines.
                line(pixels, width, height, color, (x, y + 3), (x + 11, y + 3));
                line(
                    pixels,
                    width,
                    height,
                    color,
                    (x + 11, y + 3),
                    (x + 11, y + 14),
                );
                line(pixels, width, height, color, (x + 11, y + 14), (x, y + 14));
                line(pixels, width, height, color, (x, y + 14), (x, y + 3));
                line(
                    pixels,
                    width,
                    height,
                    color,
                    (x + 5, y + 7),
                    (x + 16, y + 7),
                );
                line(
                    pixels,
                    width,
                    height,
                    color,
                    (x + 16, y + 7),
                    (x + 16, y + 18),
                );
                line(
                    pixels,
                    width,
                    height,
                    color,
                    (x + 16, y + 18),
                    (x + 5, y + 18),
                );
            } else {
                // A four-pane icon represents tiling a floating window.
                line(pixels, width, height, color, (x, y + 3), (x + 16, y + 3));
                line(
                    pixels,
                    width,
                    height,
                    color,
                    (x + 16, y + 3),
                    (x + 16, y + 19),
                );
                line(pixels, width, height, color, (x + 16, y + 19), (x, y + 19));
                line(pixels, width, height, color, (x, y + 19), (x, y + 3));
                line(
                    pixels,
                    width,
                    height,
                    color,
                    (x + 8, y + 3),
                    (x + 8, y + 19),
                );
                line(pixels, width, height, color, (x, y + 11), (x + 16, y + 11));
            }
        }
        2 => {
            line(
                pixels,
                width,
                height,
                color,
                (x + 1, y + 4),
                (x + 15, y + 4),
            );
            line(
                pixels,
                width,
                height,
                color,
                (x + 15, y + 4),
                (x + 15, y + 18),
            );
            line(
                pixels,
                width,
                height,
                color,
                (x + 15, y + 18),
                (x + 1, y + 18),
            );
            line(
                pixels,
                width,
                height,
                color,
                (x + 1, y + 18),
                (x + 1, y + 4),
            );
        }
        _ => {
            line(
                pixels,
                width,
                height,
                color,
                (x + 3, y + 5),
                (x + 15, y + 17),
            );
            line(
                pixels,
                width,
                height,
                color,
                (x + 15, y + 5),
                (x + 3, y + 17),
            );
        }
    }
}

struct Entry {
    width: i32,
    height: i32,
    hovered: Option<u8>,
    fullscreen: bool,
    maximized: bool,
    tiled: bool,
    buffer: MemoryRenderBuffer,
}

/// This cache is created only when a menu first becomes visible and retains at
/// most eight modest-sized variants for hover and window-state changes.
pub(super) struct WindowMenus {
    fonts: FontSystem,
    glyphs: SwashCache,
    entries: VecDeque<Entry>,
    background: [u8; 4],
    hover: [u8; 4],
    foreground: [u8; 4],
}

impl Default for WindowMenus {
    fn default() -> Self {
        let mut scene = Scene::new(Element::new(1, Widget::Panel)).expect("valid menu scene");
        scene
            .set_stylesheet("panel { background-color: var(--surface); color: var(--text); }")
            .expect("valid menu style");
        let style = scene
            .computed_style(ElementId(1))
            .expect("menu style")
            .style;
        let background = style
            .background
            .and_then(|background| background.fill)
            .expect("menu background");
        let foreground = style.text.color;
        let mut hover_scene =
            Scene::new(Element::new(2, Widget::Panel)).expect("valid hover scene");
        hover_scene
            .set_stylesheet("panel { background-color: var(--surface-hover); color: var(--text); }")
            .expect("valid hover style");
        let hover = hover_scene
            .computed_style(ElementId(2))
            .expect("hover style")
            .style
            .background
            .and_then(|background| background.fill)
            .expect("hover background");
        Self {
            fonts: FontSystem::new(),
            glyphs: SwashCache::new(),
            entries: VecDeque::new(),
            background: [background.red, background.green, background.blue, 255],
            hover: [hover.red, hover.green, hover.blue, 255],
            foreground: [foreground.red, foreground.green, foreground.blue, 255],
        }
    }
}

impl WindowMenus {
    pub(super) fn buffer(
        &mut self,
        width: i32,
        height: i32,
        hovered: Option<u8>,
        fullscreen: bool,
        maximized: bool,
        tiled: bool,
    ) -> Option<&MemoryRenderBuffer> {
        // Bound both allocations and raster work even if a malformed geometry
        // reaches the compositor from a stale or incorrectly sized menu state.
        let width = width.clamp(160, MAX_WIDTH);
        let height = height.clamp(ROW_HEIGHT * 4, MAX_HEIGHT);
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.width == width
                && entry.height == height
                && entry.hovered == hovered
                && entry.fullscreen == fullscreen
                && entry.maximized == maximized
                && entry.tiled == tiled
        }) {
            let entry = self.entries.remove(index)?;
            self.entries.push_front(entry);
            return self.entries.front().map(|entry| &entry.buffer);
        }

        let mut pixels = vec![0; width as usize * height as usize * 4];
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&bgra(self.background));
        }
        // A narrow accent on the left and clear row separation keep the popover
        // legible against both light and dark desktop themes.
        for row in 0..4_i32 {
            let top = INSET + row * ROW_HEIGHT;
            if hovered == Some(row as u8) {
                for y in top..(top + ROW_HEIGHT).min(height - INSET) {
                    for x in 4..width - 4 {
                        let offset = ((y * width + x) * 4) as usize;
                        pixels[offset..offset + 4].copy_from_slice(&bgra(self.hover));
                    }
                }
            }
            if row > 0 {
                let y = top;
                for x in 16..width - 16 {
                    let offset = ((y * width + x) * 4) as usize;
                    pixels[offset..offset + 4].copy_from_slice(&bgra([
                        self.foreground[0],
                        self.foreground[1],
                        self.foreground[2],
                        30,
                    ]));
                }
            }
        }

        let labels = menu_labels(fullscreen, maximized, tiled);
        let color = TextColor::rgba(
            self.foreground[0],
            self.foreground[1],
            self.foreground[2],
            255,
        );
        for (row, label) in labels.into_iter().enumerate() {
            let mut text = Buffer::new(&mut self.fonts, Metrics::new(13.0, 18.0));
            text.set_size(Some((width - 64) as f32), Some(ROW_HEIGHT as f32));
            text.set_text(label, &Attrs::new(), Shaping::Advanced, None);
            let row_top = INSET + row as i32 * ROW_HEIGHT;
            text.draw(&mut self.fonts, &mut self.glyphs, color, |x, y, w, h, c| {
                let [r, g, b, a] = c.as_rgba();
                for yy in y..y + h as i32 {
                    for xx in x..x + w as i32 {
                        let (xx, yy) = (xx + 48, yy + row_top + 6);
                        if xx < 0 || xx >= width || !(0..height).contains(&yy) {
                            continue;
                        }
                        let index = ((yy * width + xx) * 4) as usize;
                        let alpha = u32::from(a);
                        for (channel, value) in [(2, r), (1, g), (0, b)] {
                            pixels[index + channel] = ((u32::from(value) * alpha
                                + u32::from(pixels[index + channel]) * (255 - alpha))
                                / 255) as u8;
                        }
                    }
                }
            });
        }
        for row in 0..4_usize {
            let top = INSET + row as i32 * ROW_HEIGHT + 5;
            icon(&mut pixels, width, height, top, row, tiled, self.foreground);
        }
        // Close icon uses the same warm accent as the titlebar close control.
        if hovered == Some(3) {
            let top = INSET + 3 * ROW_HEIGHT + 5;
            icon(
                &mut pixels,
                width,
                height,
                top,
                3,
                tiled,
                [192, 92, 92, 255],
            );
        }

        let buffer = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (width, height),
            1,
            Transform::Normal,
            None,
        );
        self.entries.push_front(Entry {
            width,
            height,
            hovered,
            fullscreen,
            maximized,
            tiled,
            buffer,
        });
        while self.entries.len() > CACHE_ENTRIES {
            self.entries.pop_back();
        }
        self.entries.front().map(|entry| &entry.buffer)
    }
}

fn menu_labels(fullscreen: bool, maximized: bool, tiled: bool) -> [&'static str; 4] {
    [
        "Minimize",
        if tiled { "Float window" } else { "Tile window" },
        if fullscreen {
            "Exit fullscreen"
        } else if maximized {
            "Restore"
        } else {
            "Maximize"
        },
        "Close",
    ]
}

#[cfg(test)]
mod tests {
    use super::menu_labels;

    #[test]
    fn fullscreen_menu_offers_exit_fullscreen_instead_of_maximize() {
        assert_eq!(menu_labels(true, true, false)[2], "Exit fullscreen");
        assert_eq!(menu_labels(false, true, false)[2], "Restore");
        assert_eq!(menu_labels(false, false, true)[2], "Maximize");
    }
}
