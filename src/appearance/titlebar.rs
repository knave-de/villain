//! Small software raster adapter for compositor titlebars. No WGPU renderer is used.
use cosmic_text::{Attrs, Buffer, Color as TextColor, FontSystem, Metrics, Shaping, SwashCache};
use knave_ui::toolkit::{Element, ElementId, Scene, Widget};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    desktop::Window,
    utils::Transform,
};
use std::collections::VecDeque;

struct Entry {
    title: String,
    width: i32,
    focused: bool,
    hovered: Option<u8>,
    maximized: bool,
    tiled: bool,
    buffer: MemoryRenderBuffer,
}
pub(super) struct Titlebars {
    fonts: FontSystem,
    glyphs: SwashCache,
    entries: VecDeque<Entry>,
    background: [u8; 4],
    foreground: [u8; 4],
}
impl Default for Titlebars {
    fn default() -> Self {
        let mut scene = Scene::new(Element::new(1, Widget::Panel)).expect("valid titlebar scene");
        scene
            .set_stylesheet("panel { background-color: var(--surface-hover); color: var(--text); }")
            .expect("valid titlebar style");
        let style = scene
            .computed_style(ElementId(1))
            .expect("titlebar style")
            .style;
        let bg = style
            .background
            .and_then(|b| b.fill)
            .expect("titlebar background");
        let fg = style.text.color;
        Self {
            fonts: FontSystem::new(),
            glyphs: SwashCache::new(),
            entries: VecDeque::new(),
            background: [bg.red, bg.green, bg.blue, 255],
            foreground: [fg.red, fg.green, fg.blue, 255],
        }
    }
}
impl Titlebars {
    pub(super) fn buffer(
        &mut self,
        window: &Window,
        width: i32,
        focused: bool,
        hovered: Option<u8>,
        maximized: bool,
        tiled: bool,
    ) -> Option<&MemoryRenderBuffer> {
        let width = width.clamp(1, 8192);
        let title = window
            .toplevel()
            .map(|surface| {
                smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
                    states
                        .data_map
                        .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                        .and_then(|data| data.lock().ok().and_then(|a| a.title.clone()))
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default();
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.title == title
                && entry.width == width
                && entry.focused == focused
                && entry.hovered == hovered
                && entry.maximized == maximized
                && entry.tiled == tiled
        }) {
            let entry = self.entries.remove(index)?;
            self.entries.push_front(entry);
            return self.entries.front().map(|e| &e.buffer);
        }
        let mut bg = self.background;
        if !focused {
            for value in &mut bg[..3] {
                *value = (*value as f32 * 0.8) as u8;
            }
        }
        let mut pixels = vec![0u8; width as usize * 34 * 4];
        for px in pixels.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&bg);
        }
        if let Some(index) = hovered {
            let right = width - i32::from(index) * 24;
            let color = if index == 0 {
                [167, 91, 91, 255]
            } else {
                [70, 77, 85, 255]
            };
            for y in 5..29 {
                for x in (right - 23).max(0)..right.min(width) {
                    let i = (y as usize * width as usize + x as usize) * 4;
                    pixels[i..i + 4].copy_from_slice(&color);
                }
            }
        }
        let mut text = Buffer::new(&mut self.fonts, Metrics::new(13.0, 18.0));
        text.set_size(Some((width - 130).max(1) as f32), Some(20.0));
        text.set_text(&title, &Attrs::new(), Shaping::Advanced, None);
        let color = TextColor::rgba(
            self.foreground[0],
            self.foreground[1],
            self.foreground[2],
            255,
        );
        text.draw(&mut self.fonts, &mut self.glyphs, color, |x, y, w, h, c| {
            let [r, g, b, a] = c.as_rgba();
            for yy in y..y + h as i32 {
                for xx in x..x + w as i32 {
                    let (xx, yy) = (xx + 12, yy + 7);
                    if xx < 0 || xx >= width - 112 || !(0..34).contains(&yy) {
                        continue;
                    }
                    let i = (yy as usize * width as usize + xx as usize) * 4;
                    let alpha = u32::from(a);
                    for (n, v) in [r, g, b].into_iter().enumerate() {
                        pixels[i + n] = ((u32::from(v) * alpha
                            + u32::from(pixels[i + n]) * (255 - alpha))
                            / 255) as u8;
                    }
                }
            }
        });
        // Four standard window-control glyphs, drawn into the same bounded buffer.
        let mut point = |x: i32, y: i32| {
            if x >= 0 && x < width && (0..34).contains(&y) {
                let i = (y as usize * width as usize + x as usize) * 4;
                pixels[i..i + 4].copy_from_slice(&self.foreground);
            }
        };
        for x in 0..10 {
            point(width - 89 + x, 19);
            point(width - 65 + x, 12);
            point(width - 65 + x, 21);
            point(width - 41 + x, 12);
            point(width - 41 + x, 21);
            point(width - 17 + x, 12 + x);
            point(width - 17 + x, 21 - x);
        }
        for y in 12..22 {
            point(width - 65, y);
            point(width - 56, y);
            point(width - 41, y);
            point(width - 32, y);
        }
        if maximized {
            for x in 0..7 {
                point(width - 39 + x, 10);
            }
            for y in 10..17 {
                point(width - 33, y);
            }
        }
        if !tiled {
            for x in 0..7 {
                point(width - 63 + x, 10);
            }
            for y in 10..17 {
                point(width - 57, y);
            }
        }
        let buffer = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (width, 34),
            1,
            Transform::Normal,
            None,
        );
        self.entries.push_front(Entry {
            title,
            width,
            focused,
            hovered,
            maximized,
            tiled,
            buffer,
        });
        while self.entries.len() > 8 {
            self.entries.pop_back();
        }
        self.entries.front().map(|entry| &entry.buffer)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_theme_resolves_for_titlebar() {
        let titlebars = super::Titlebars::default();
        assert_ne!(titlebars.background, titlebars.foreground);
    }
}
