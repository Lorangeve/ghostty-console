use crate::fonts::{Fonts, Glyph};
use crate::{rime::Update, terminal::Frame};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::Path,
};
pub struct Renderer {
    fonts: Fonts,
    ime_fonts: Fonts,
    ime_cache: HashMap<char, Glyph>,
    cache: HashMap<char, Glyph>,
    pub panel_visible: bool,
    pub help_visible: bool,
    pub login_screen: bool,
    pub buffers: crate::buffers::Buffers,
    pub selection: Option<crate::interaction::Selection>,
    pub pointer: Option<(usize, usize)>,
    pointer_sprite: crate::cursor::Cursor,
    background: u32,
    cursor_color: u32,
    pub ime_display: crate::settings::ImeDisplay,
}
impl Renderer {
    pub fn new() -> Result<Self, String> {
        Self::create(false)
    }
    pub fn greeter() -> Result<Self, String> {
        Self::create(true)
    }
    fn create(greeter: bool) -> Result<Self, String> {
        let settings = crate::settings::get()?;
        let mut fonts = if greeter {
            Fonts::system()?
        } else {
            Fonts::new()?
        };
        let mut ime_fonts = if greeter {
            Fonts::system()?
        } else {
            Fonts::ime().or_else(|_| Fonts::new())?
        };
        let height = fonts.cell_height.max(ime_fonts.cell_height);
        fonts.baseline += (height - fonts.cell_height) / 2;
        ime_fonts.baseline += (height - ime_fonts.cell_height) / 2;
        fonts.cell_height = height;
        ime_fonts.cell_height = height;
        let pointer_sprite = crate::cursor::Cursor::arrow(height);
        Ok(Self {
            pointer_sprite,
            fonts,
            ime_fonts,
            cache: HashMap::new(),
            background: settings.background.unwrap_or(0x141820),
            cursor_color: settings.cursor_color,
            ime_display: settings.ime_display,
            panel_visible: settings.status_bar_visible,
            help_visible: false,
            login_screen: false,
            buffers: crate::buffers::Buffers::default(),
            selection: None,
            pointer: None,
            ime_cache: HashMap::new(),
        })
    }
    pub fn cell_width(&self) -> usize {
        self.fonts.cell_width
    }
    pub fn cell_height(&self) -> usize {
        self.fonts.cell_height
    }
    pub fn frame(
        &mut self,
        frame: &Frame,
        width: usize,
        height: usize,
        ime: Option<&Update>,
        enabled: bool,
    ) -> Vec<u32> {
        let mut pixels = vec![self.background; width * height];
        let cols = usize::from(frame.cols);
        self.images(&mut pixels, width, frame, 0);
        for (index, cell) in frame.cells.iter().enumerate() {
            rect(
                &mut pixels,
                width,
                index % cols * self.fonts.cell_width,
                index / cols * self.fonts.cell_height,
                self.fonts.cell_width,
                self.fonts.cell_height,
                cell.bg,
            );
        }
        self.images(&mut pixels, width, frame, 1);
        for (index, cell) in frame.cells.iter().enumerate() {
            if cell.width == 0 {
                continue;
            }
            let x = index % cols * self.fonts.cell_width;
            let y = index / cols * self.fonts.cell_height;
            for ch in cell.text().chars() {
                self.glyph(
                    &mut pixels,
                    width,
                    x,
                    y,
                    ch,
                    cell.fg,
                    cell.width as usize * self.fonts.cell_width,
                );
            }
            if cell.flags & 2 != 0 {
                rect(
                    &mut pixels,
                    width,
                    x,
                    y + self.fonts.cell_height - 3,
                    cell.width as usize * self.fonts.cell_width,
                    1,
                    cell.fg,
                );
            }
        }
        if let Some((x, y)) = frame.cursor {
            rect(
                &mut pixels,
                width,
                x * self.fonts.cell_width,
                (y + 1) * self.fonts.cell_height - 2,
                self.fonts.cell_width,
                2,
                self.cursor_color,
            );
        }
        self.images(&mut pixels, width, frame, 2);
        if let Some(selection) = self.selection {
            for row in 0..usize::from(frame.rows) {
                for col in 0..cols {
                    if selection.contains(col, row, cols) {
                        let x = col * self.cell_width();
                        let y = row * self.cell_height();
                        for py in y..(y + self.cell_height()).min(height) {
                            for px in x..(x + self.cell_width()).min(width) {
                                let p = &mut pixels[py * width + px];
                                *p = ((*p & 0xfefefe) >> 1) + 0x254777;
                            }
                        }
                    }
                }
            }
        }
        self.draw_buffer_window(&mut pixels, width, height);
        let status_y = height.saturating_sub(self.cell_height());
        if self.panel_visible {
            self.draw_status(&mut pixels, width, status_y, frame, enabled);
        }
        let echo_y =
            height.saturating_sub(self.cell_height() * (1 + usize::from(self.panel_visible)));
        rect(
            &mut pixels,
            width,
            0,
            echo_y,
            width,
            self.cell_height(),
            0x141820,
        );
        let composing = self.buffers.window.buffer == crate::buffers::BufferId::Terminal
            && enabled
            && ime.is_some_and(|i| !i.preedit.is_empty() || !i.candidates.is_empty());
        if composing && self.ime_display == crate::settings::ImeDisplay::Inline {
            self.draw_ime_inline(&mut pixels, width, echo_y, ime);
        } else {
            if let Some(message) = self.buffers.echo() {
                let text = format!("{}: {}", message.source, message.text.replace('\n', " · "));
                self.ui_text(&mut pixels, width, 8, echo_y, &text, 0xd9dfeb);
            }
            if composing && let Some(ime) = ime {
                self.draw_ime_popup(&mut pixels, width, height, frame, ime);
            }
        }
        self.draw_help(&mut pixels, width, height);
        self.draw_pointer(&mut pixels, width);
        pixels
    }
    pub fn panel_rows(&self, _enabled: bool, _ime: Option<&Update>) -> usize {
        usize::from(self.panel_visible) + 1
    }
    fn draw_buffer_window(&mut self, pixels: &mut [u32], width: usize, height: usize) {
        if self.buffers.window.buffer != crate::buffers::BufferId::Messages {
            return;
        }
        let bottom = height.saturating_sub(self.panel_rows(false, None) * self.cell_height());
        rect(pixels, width, 0, 0, width, bottom, self.background);
        let count = bottom / self.cell_height();
        let all = self.buffers.lines();
        let end = all.len().saturating_sub(self.buffers.window.scroll);
        let start = end.saturating_sub(count);
        let lines: Vec<String> = all[start..end]
            .iter()
            .map(|s| s.chars().skip(self.buffers.window.horizontal).collect())
            .collect();
        if lines.is_empty() {
            self.ui_text(pixels, width, 8, 0, "*Messages* — 暂无消息", 0x8b94aa);
        }
        for (row, line) in lines.iter().enumerate() {
            self.ui_text(pixels, width, 8, row * self.cell_height(), line, 0xd9dfeb);
        }
    }
    fn draw_ime_inline(
        &mut self,
        pixels: &mut [u32],
        width: usize,
        y: usize,
        ime: Option<&Update>,
    ) {
        let line = self.cell_height();
        rect(pixels, width, 0, y, width, line, 0x171b29);
        let Some(ime) = ime else {
            return;
        };
        if ime.preedit.is_empty() && ime.candidates.is_empty() {
            return;
        }
        let right = width.saturating_sub(8);
        let mut x = 8;
        x = self.ime_text_limited(pixels, width, x, y, &ime.preedit, 0xffe0a0, right);
        let offset = ime.cursor.min(ime.preedit.len());
        if ime.preedit.is_char_boundary(offset) {
            let cursor = (8 + self.ime_measure(&ime.preedit[..offset])).min(right);
            rect(
                pixels,
                width,
                cursor,
                y + 3,
                1,
                line.saturating_sub(6),
                self.cursor_color,
            );
        }
        x += 14;
        if x < right {
            rect(
                pixels,
                width,
                x,
                y + 7,
                1,
                line.saturating_sub(14),
                0x3b4356,
            );
        }
        x += 14;
        for (i, candidate) in ime.candidates.iter().enumerate() {
            let label = if candidate.label.is_empty() {
                (i + 1).to_string()
            } else {
                candidate.label.clone()
            };
            let text = format!("{label}.{} {}", candidate.text, candidate.comment);
            let color = if i == ime.highlighted {
                0xffe0a0
            } else {
                0xdddddd
            };
            x = self.ime_text_limited(pixels, width, x, y, &text, color, right) + 20;
            if x >= right {
                break;
            }
        }
    }
    fn draw_ime_popup(
        &mut self,
        pixels: &mut [u32],
        width: usize,
        height: usize,
        frame: &Frame,
        ime: &Update,
    ) {
        let line = self.cell_height();
        let entries: Vec<String> = ime
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let label = if c.label.is_empty() {
                    (i + 1).to_string()
                } else {
                    c.label.clone()
                };
                format!("{label}.{} {}", c.text, c.comment)
            })
            .collect();
        let content_width = entries
            .iter()
            .map(|s| self.ime_measure(s) + 20)
            .sum::<usize>()
            .max(self.ime_measure(&ime.preedit));
        let popup_width = (content_width + 24).max(180).min(width.saturating_sub(16));
        let popup_height = (line * 2 + 12).min(height);
        let caret = frame
            .caret
            .unwrap_or((0, usize::from(frame.rows).saturating_sub(1)));
        let x = (caret.0 * self.cell_width()).min(width.saturating_sub(popup_width + 8));
        let viewport_bottom = usize::from(frame.rows) * line;
        let below = (caret.1 + 1) * line + 3;
        let y = if below + popup_height <= viewport_bottom {
            below
        } else {
            (caret.1 * line).saturating_sub(popup_height + 3)
        };
        super::status::rounded(
            pixels,
            width,
            x + 2,
            y + 3,
            popup_width,
            popup_height,
            0x080b12,
        );
        super::status::rounded(pixels, width, x, y, popup_width, popup_height, 0x202739);
        rect(
            pixels,
            width,
            x + 3,
            y,
            popup_width.saturating_sub(6),
            1,
            0x718864,
        );
        let left = x + 10;
        let right = x + popup_width.saturating_sub(10);
        self.ime_text_limited(pixels, width, left, y + 4, &ime.preedit, 0xffe0a0, right);
        let offset = ime.cursor.min(ime.preedit.len());
        if ime.preedit.is_char_boundary(offset) {
            let cursor = (left + self.ime_measure(&ime.preedit[..offset])).min(right);
            rect(
                pixels,
                width,
                cursor,
                y + 5,
                1,
                line.saturating_sub(2),
                self.cursor_color,
            );
        }
        let mut cursor = left;
        for (i, entry) in entries.iter().enumerate() {
            let needed = self.ime_measure(entry) + 12;
            if i == ime.highlighted {
                super::status::rounded(
                    pixels,
                    width,
                    cursor.saturating_sub(3),
                    y + line + 4,
                    needed.min(right.saturating_sub(cursor) + 3),
                    line.saturating_sub(5),
                    0x354134,
                );
            }
            let color = if i == ime.highlighted {
                0xffe0a0
            } else {
                0xdddddd
            };
            cursor =
                self.ime_text_limited(pixels, width, cursor, y + line + 3, entry, color, right)
                    + 20;
            if cursor >= right {
                break;
            }
        }
    }
    pub(crate) fn ui_measure(&mut self, text: &str) -> usize {
        text.chars()
            .map(|ch| {
                self.cache
                    .entry(ch)
                    .or_insert_with(|| self.fonts.rasterize(ch))
                    .advance
            })
            .sum()
    }
    pub(crate) fn ui_text(
        &mut self,
        pixels: &mut [u32],
        width: usize,
        mut x: usize,
        y: usize,
        text: &str,
        color: u32,
    ) -> usize {
        for ch in text.chars() {
            let glyph = self
                .cache
                .entry(ch)
                .or_insert_with(|| self.fonts.rasterize(ch));
            if x + glyph.advance > width {
                break;
            }
            blit(
                pixels,
                width,
                x,
                y,
                glyph,
                self.fonts.baseline,
                self.fonts.cell_height,
                glyph
                    .advance
                    .max((glyph.left + glyph.width as i32).max(0) as usize),
                color,
            );
            x += glyph.advance;
        }
        x
    }
    fn images(&self, pixels: &mut [u32], width: usize, frame: &Frame, layer: i32) {
        let viewport_height = usize::from(frame.rows) * self.cell_height();
        for image in frame.images.iter().filter(|i| i.layer == layer) {
            if image.dest_width == 0
                || image.dest_height == 0
                || image.source_width == 0
                || image.source_height == 0
            {
                continue;
            }
            if image
                .source_x
                .checked_add(image.source_width)
                .is_none_or(|v| v > image.width)
                || image
                    .source_y
                    .checked_add(image.source_height)
                    .is_none_or(|v| v > image.height)
            {
                continue;
            }
            let left = i64::from(image.col) * self.cell_width() as i64 + i64::from(image.offset_x);
            let top = i64::from(image.row) * self.cell_height() as i64 + i64::from(image.offset_y);
            let right = (left + i64::from(image.dest_width))
                .min(width as i64)
                .max(0) as usize;
            let bottom = (top + i64::from(image.dest_height))
                .min(viewport_height.min(pixels.len() / width) as i64)
                .max(0) as usize;
            let bpp = match image.format {
                0 => 3,
                1 => 4,
                3 => 2,
                4 => 1,
                _ => continue,
            };
            for y in top.max(0) as usize..bottom {
                for x in left.max(0) as usize..right {
                    let sx = i64::from(image.source_x)
                        + (x as i64 - left) * i64::from(image.source_width)
                            / i64::from(image.dest_width);
                    let sy = i64::from(image.source_y)
                        + (y as i64 - top) * i64::from(image.source_height)
                            / i64::from(image.dest_height);
                    let offset = (sy as usize * image.width as usize + sx as usize) * bpp;
                    if offset + bpp > image.pixels.len() {
                        continue;
                    }
                    let bytes = &image.pixels[offset..offset + bpp];
                    let (r, g, b, a) = match image.format {
                        0 => (bytes[0], bytes[1], bytes[2], 255),
                        1 => (bytes[0], bytes[1], bytes[2], bytes[3]),
                        3 => (bytes[0], bytes[0], bytes[0], bytes[1]),
                        _ => (bytes[0], bytes[0], bytes[0], 255),
                    };
                    let fg = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
                    let pixel = &mut pixels[y * width + x];
                    let mut color = 0;
                    for shift in [0, 8, 16] {
                        color |= ((((fg >> shift) & 255) * u32::from(a)
                            + ((*pixel >> shift) & 255) * u32::from(255 - a))
                            / 255)
                            << shift;
                    }
                    *pixel = color;
                }
            }
        }
    }
    fn draw_pointer(&self, pixels: &mut [u32], width: usize) {
        if let Some(position) = self.pointer {
            self.pointer_sprite.draw(pixels, width, position);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn glyph(
        &mut self,
        pixels: &mut [u32],
        width: usize,
        x: usize,
        y: usize,
        ch: char,
        color: u32,
        cell_width: usize,
    ) {
        if ch == ' ' || ch == '\0' {
            return;
        }
        if self.cache.len() > 8192 {
            self.cache.clear();
        }
        let glyph = self
            .cache
            .entry(ch)
            .or_insert_with(|| self.fonts.rasterize(ch));
        blit(
            pixels,
            width,
            x,
            y,
            glyph,
            self.fonts.baseline,
            self.fonts.cell_height,
            cell_width,
            color,
        );
    }
    fn ime_measure(&mut self, text: &str) -> usize {
        text.chars()
            .map(|ch| {
                self.ime_cache
                    .entry(ch)
                    .or_insert_with(|| self.ime_fonts.rasterize(ch))
                    .advance
            })
            .sum()
    }
    #[allow(clippy::too_many_arguments)]
    fn ime_text_limited(
        &mut self,
        pixels: &mut [u32],
        width: usize,
        mut x: usize,
        y: usize,
        text: &str,
        color: u32,
        right: usize,
    ) -> usize {
        if self.ime_cache.len() > 8192 {
            self.ime_cache.clear();
        }
        let mut previous = x;
        for ch in text.chars() {
            let glyph = self
                .ime_cache
                .entry(ch)
                .or_insert_with(|| self.ime_fonts.rasterize(ch));
            if x + glyph.advance > right {
                return right;
            }
            let origin = if glyph.advance == 0 { previous } else { x };
            let extent = glyph
                .advance
                .max((glyph.left + glyph.width as i32).max(0) as usize)
                .min(right.saturating_sub(origin));
            blit(
                pixels,
                width,
                origin,
                y,
                glyph,
                self.ime_fonts.baseline,
                self.fonts.cell_height,
                extent,
                color,
            );
            if glyph.advance > 0 {
                previous = x;
                x += glyph.advance;
            }
        }
        x
    }
}
#[allow(clippy::too_many_arguments)]
fn blit(
    pixels: &mut [u32],
    width: usize,
    x: usize,
    y: usize,
    glyph: &Glyph,
    baseline: usize,
    line_height: usize,
    extent: usize,
    color: u32,
) {
    let top = y as isize + baseline as isize - glyph.top as isize;
    let left = x as isize + glyph.left as isize;
    for row in 0..glyph.height {
        for col in 0..glyph.width {
            let px = left + col as isize;
            let py = top + row as isize;
            if px < x as isize
                || px >= (x + extent).min(width) as isize
                || py < y as isize
                || py >= (y + line_height) as isize
            {
                continue;
            }
            let index = py as usize * width + px as usize;
            if index >= pixels.len() {
                continue;
            }
            let source = row * glyph.width + col;
            let alpha = u32::from(glyph.bitmap[source]);
            let fg = glyph.colors.as_ref().map(|c| c[source]).unwrap_or(color);
            let bg = pixels[index];
            let mut rgb = 0;
            for shift in [0, 8, 16] {
                rgb |= ((((fg >> shift) & 255) * alpha + ((bg >> shift) & 255) * (255 - alpha))
                    / 255)
                    << shift;
            }
            pixels[index] = rgb;
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn rect(
    pixels: &mut [u32],
    width: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: u32,
) {
    let height = pixels.len() / width;
    for row in y.min(height)..y.saturating_add(h).min(height) {
        let start = row * width + x.min(width);
        let end = row * width + x.saturating_add(w).min(width);
        pixels[start..end].fill(color);
    }
}
pub fn save_ppm(path: &Path, width: usize, height: usize, pixels: &[u32]) -> io::Result<()> {
    let mut file = io::BufWriter::new(fs::File::create(path)?);
    write!(file, "P6\n{width} {height}\n255\n")?;
    for p in pixels {
        file.write_all(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8])?;
    }
    file.flush()
}
