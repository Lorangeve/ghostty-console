//! Role-aware Fontconfig/FreeType selection. Private-use glyphs must use the UI's own font.
use freetype::{Face, Library, bitmap::PixelMode, face::LoadFlag};
use std::{collections::HashSet, env, fs, path::PathBuf, process::Command};
#[derive(Default)]
pub struct Glyph {
    pub width: usize,
    pub height: usize,
    pub left: i32,
    pub top: i32,
    pub advance: usize,
    pub bitmap: Vec<u8>,
    pub colors: Option<Vec<u32>>,
}
#[derive(Clone)]
struct Spec {
    path: String,
    index: isize,
    family: String,
    charset: Vec<(u32, u32)>,
}
impl Spec {
    fn covers(&self, ch: char) -> bool {
        let code = ch as u32;
        self.charset.iter().any(|(a, b)| *a <= code && code <= *b)
    }
}
pub struct Fonts {
    library: Library,
    faces: Vec<Face>,
    specs: Vec<Spec>,
    loaded: Vec<Option<usize>>,
    size: f32,
    role: &'static str,
    pub cell_width: usize,
    pub cell_height: usize,
    pub baseline: usize,
}
fn config_file(relative: &str) -> String {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .and_then(|d| fs::read_to_string(d.join(relative)).ok())
        .unwrap_or_default()
}
fn query(program: &str, pattern: Option<&str>, sorted: bool) -> Result<Vec<Spec>, String> {
    let mut cmd = Command::new(program);
    if sorted {
        cmd.arg("-s");
    }
    cmd.args(["-f", "%{file}|%{index}|%{family}|%{charset}\n"]);
    if let Some(pattern) = pattern {
        cmd.arg(pattern);
    }
    let result = cmd.output().map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(format!("{program} failed"));
    }
    let text = String::from_utf8(result.stdout).map_err(|e| e.to_string())?;
    text.lines()
        .map(|line| {
            let fields: Vec<_> = line.splitn(4, '|').collect();
            if fields.len() != 4 {
                return Err("Invalid Fontconfig font record".into());
            }
            let charset = fields[3]
                .split_whitespace()
                .filter_map(|range| {
                    let (a, b) = range.split_once('-').unwrap_or((range, range));
                    Some((
                        u32::from_str_radix(a, 16).ok()?,
                        u32::from_str_radix(b, 16).ok()?,
                    ))
                })
                .collect();
            Ok(Spec {
                path: fields[0].into(),
                index: fields[1].parse::<isize>().map_err(|e| e.to_string())?,
                family: fields[2].into(),
                charset,
            })
        })
        .collect()
}
fn load(library: &Library, spec: &Spec, size: f32) -> Result<Face, String> {
    let face = library
        .new_face(&spec.path, spec.index)
        .map_err(|e| format!("Cannot load {}: {e}", spec.path))?;
    if let Err(e) = face.set_char_size(0, (size * 64.).round() as isize, 72, 72) {
        if !face.has_fixed_sizes() {
            return Err(e.to_string());
        }
        face.select_size(0).map_err(|e| e.to_string())?;
    }
    Ok(face)
}
impl Fonts {
    pub fn system() -> Result<Self, String> {
        Self::build("monospace", 18., "greeter")
    }
    pub fn new() -> Result<Self, String> {
        let settings = crate::settings::get()?;
        let config = config_file("ghostty/config");
        let family = env::var("GHOSTTY_CONSOLE_FONT_FAMILY")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| settings.font_family.clone())
            .or_else(|| setting(&config, "font-family"))
            .unwrap_or_else(|| "FiraMono Nerd Font Mono".into());
        let size = if let Ok(value) = env::var("GHOSTTY_CONSOLE_FONT_SIZE") {
            value
                .parse::<f32>()
                .map_err(|_| "Invalid font pixel size")?
        } else if let Some(value) = settings.font_size {
            value * 96. / 72.
        } else if let Some(value) = setting(&config, "font-size") {
            value
                .parse::<f32>()
                .map_err(|_| "Invalid Ghostty font-size")?
                * 96.
                / 72.
        } else {
            18.
        };
        Self::build(&family, size, "terminal")
    }
    pub fn ime() -> Result<Self, String> {
        let settings = crate::settings::get()?;
        let config = config_file("fcitx5/conf/classicui.conf");
        let description = setting(&config, "Font").unwrap_or_else(|| "sans-serif 12".into());
        let (family, size) = description
            .rsplit_once(' ')
            .and_then(|(family, size)| size.parse::<f32>().ok().map(|s| (family, s)))
            .unwrap_or((description.as_str(), 12.));
        let family = env::var("GHOSTTY_CONSOLE_IME_FONT_FAMILY")
            .ok()
            .or_else(|| settings.ime_font_family.clone())
            .unwrap_or_else(|| family.into());
        let size = env::var("GHOSTTY_CONSOLE_IME_FONT_SIZE")
            .ok()
            .map(|s| s.parse::<f32>())
            .transpose()
            .map_err(|_| "Invalid IME pixel size")?
            .unwrap_or(settings.ime_font_size.unwrap_or(size) * 96. / 72.);
        Self::build(&family, size, "ime")
    }
    fn build(family: &str, size: f32, role: &'static str) -> Result<Self, String> {
        if !size.is_finite() || !(8. ..=96.).contains(&size) {
            return Err("Font pixel size must be between 8 and 96".into());
        }
        let installed = query("fc-list", None, false)?;
        // Existing user rules can substitute a different font even for a requested exact family.
        // Pin an installed exact family before asking Fontconfig for the fallback chain.
        let chain = query("fc-match", Some(family), true)?;
        let matches = |spec: &&Spec| {
            spec.family
                .split(',')
                .any(|name| name.eq_ignore_ascii_case(family))
        };
        let exact = chain.iter().find(matches).cloned().or_else(|| {
            installed
                .iter()
                .filter(matches)
                .min_by_key(|spec| {
                    !(spec.path.ends_with("-Regular.otf") || spec.path.ends_with("-Regular.ttf"))
                })
                .cloned()
        });
        let mut specs = Vec::new();
        if let Some(exact) = exact {
            specs.push(exact);
        }
        specs.extend(chain);
        specs.extend(installed);
        let mut seen = HashSet::new();
        specs.retain(|s| seen.insert((s.path.clone(), s.index)));
        let first = specs.first().ok_or("No fonts available")?;
        let library = Library::init().map_err(|e| e.to_string())?;
        let face = load(&library, first, size)?;
        face.load_char('M' as usize, LoadFlag::DEFAULT)
            .map_err(|e| e.to_string())?;
        let cell_width = ((face.glyph().advance().x + 63) / 64).max(1) as usize;
        let metrics = face.size_metrics().ok_or("Font has no size metrics")?;
        let ascent = ((metrics.ascender + 63) / 64).max(1) as usize;
        let descent = ((-metrics.descender + 63) / 64).max(0) as usize;
        let cell_height = (ascent + descent).max(((metrics.height + 63) / 64).max(1) as usize) + 2;
        let baseline = ascent + 1;
        eprintln!(
            "font[{role}]: requested={family:?} resolved={:?} file={} index={} pixels={size:.2} renderer=FreeType",
            first.family, first.path, first.index
        );
        eprintln!(
            "font[{role}] geometry: cell={cell_width}x{cell_height} baseline={baseline}; {} fallback candidates",
            specs.len()
        );
        let mut loaded = vec![None; specs.len()];
        loaded[0] = Some(0);
        Ok(Self {
            library,
            faces: vec![face],
            specs,
            loaded,
            size,
            role,
            cell_width,
            cell_height,
            baseline,
        })
    }
    fn face_index(&mut self, ch: char) -> usize {
        for i in 0..self.specs.len() {
            if !self.specs[i].covers(ch) {
                continue;
            }
            if let Some(index) = self.loaded[i] {
                return index;
            }
            if let Ok(face) = load(&self.library, &self.specs[i], self.size) {
                let index = self.faces.len();
                eprintln!(
                    "font[{}] fallback: U+{:04X} -> {:?} ({})",
                    self.role, ch as u32, self.specs[i].family, self.specs[i].path
                );
                self.faces.push(face);
                self.loaded[i] = Some(index);
                return index;
            }
        }
        eprintln!(
            "font[{}]: no installed font covers U+{:04X}",
            self.role, ch as u32
        );
        0
    }
    pub fn rasterize(&mut self, ch: char) -> Glyph {
        let index = self.face_index(ch);
        let face = &self.faces[index];
        if let Err(error) = face.load_char(
            ch as usize,
            LoadFlag::RENDER | LoadFlag::TARGET_NORMAL | LoadFlag::COLOR,
        ) {
            eprintln!("Cannot render {ch:?}: {error}");
            return Glyph::default();
        }
        let slot = face.glyph();
        let bitmap = slot.bitmap();
        let width = bitmap.width().max(0) as usize;
        let height = bitmap.rows().max(0) as usize;
        let pitch = bitmap.pitch();
        let source = bitmap.buffer();
        let mut pixels = vec![0; width * height];
        let mode = bitmap.pixel_mode().ok();
        let mut colors = if mode == Some(PixelMode::Bgra) {
            Some(vec![0; width * height])
        } else {
            None
        };
        for y in 0..height {
            let row = if pitch >= 0 { y } else { height - 1 - y };
            for x in 0..width {
                let offset = row * pitch.unsigned_abs() as usize;
                let index = y * width + x;
                pixels[index] = match mode {
                    Some(PixelMode::Gray) => *source.get(offset + x).unwrap_or(&0),
                    Some(PixelMode::Mono) => {
                        if source.get(offset + x / 8).unwrap_or(&0) & (0x80 >> (x % 8)) != 0 {
                            255
                        } else {
                            0
                        }
                    }
                    Some(PixelMode::Bgra) => {
                        let p = offset + x * 4;
                        let a = u32::from(source[p + 3]);
                        if a > 0 {
                            colors.as_mut().unwrap()[index] =
                                ((u32::from(source[p + 2]) * 255 / a).min(255) << 16)
                                    | ((u32::from(source[p + 1]) * 255 / a).min(255) << 8)
                                    | (u32::from(source[p]) * 255 / a).min(255);
                        }
                        a as u8
                    }
                    _ => 0,
                };
            }
        }
        let glyph = Glyph {
            width,
            height,
            left: slot.bitmap_left(),
            top: slot.bitmap_top(),
            advance: ((slot.advance().x + 63) / 64).max(0) as usize,
            bitmap: pixels,
            colors,
        };
        // Fixed-strike emoji may be much larger than the requested pixel size.
        if height > self.size.ceil() as usize * 2 && face.has_fixed_sizes() {
            scale_glyph(glyph, self.size.ceil() as usize)
        } else {
            glyph
        }
    }
}
fn scale_glyph(g: Glyph, target: usize) -> Glyph {
    if g.height == 0 {
        return g;
    }
    let scale = target as f64 / g.height as f64;
    let width = ((g.width as f64 * scale).round() as usize).max(1);
    let height = target.max(1);
    let mut bitmap = vec![0; width * height];
    let mut colors = g.colors.as_ref().map(|_| vec![0; width * height]);
    for y in 0..height {
        for x in 0..width {
            let src = (y * g.height / height) * g.width + x * g.width / width;
            let dst = y * width + x;
            bitmap[dst] = g.bitmap[src];
            if let Some(ref mut colors) = colors {
                colors[dst] = g.colors.as_ref().unwrap()[src];
            }
        }
    }
    Glyph {
        width,
        height,
        left: (g.left as f64 * scale).round() as i32,
        top: (g.top as f64 * scale).round() as i32,
        advance: (g.advance as f64 * scale).round() as usize,
        bitmap,
        colors,
    }
}
fn setting(config: &str, key: &str) -> Option<String> {
    let mut result = None;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            let value = v.trim().trim_matches('"');
            result = if value.is_empty() {
                None
            } else {
                Some(value.into())
            };
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::setting;
    #[test]
    #[ignore = "requires locally installed FiraMono Nerd Font Mono and Chinese fonts"]
    fn installed_nerd_font_and_chinese_glyphs_render() {
        let mut fonts = super::Fonts::new().unwrap();
        assert!(fonts.faces[0].family_name().unwrap().contains("FiraMono"));
        for ch in ['M', '\u{e0b0}', '\u{f120}', '\u{f07b}', '你'] {
            if ch != '你' {
                assert!(fonts.faces[0].get_char_index(ch as usize).is_some());
            }
            let glyph = fonts.rasterize(ch);
            assert!(glyph.width > 0 && glyph.height > 0, "{ch:?}");
            assert!(glyph.bitmap.iter().any(|value| *value > 0), "{ch:?}");
        }
        assert!(fonts.baseline < fonts.cell_height);
    }
    #[test]
    #[ignore = "requires this machine's 98WB-2 and Nerd Font installations"]
    fn ime_roots_do_not_use_nerd_icons_and_terminal_can_fallback() {
        let mut terminal = super::Fonts::new().unwrap();
        let mut ime = super::Fonts::ime().unwrap();
        assert_eq!(ime.specs[0].family, "98WB-2");
        assert!(ime.specs[0].path.ends_with("98WB-2.otf"));
        for ch in ['\u{f0125}', '\u{f013e}', '\u{f011f}', '\u{f008d}'] {
            assert!(ime.faces[0].get_char_index(ch as usize).is_some());
            let root = ime.rasterize(ch);
            let icon = terminal.rasterize(ch);
            assert!(root.bitmap.iter().any(|p| *p > 0));
            assert_ne!(
                root.bitmap, icon.bitmap,
                "PUA root was rendered as a Nerd icon"
            );
        }
        let fallback = terminal.face_index('𠮷');
        assert_ne!(fallback, 0);
        assert!(
            terminal.faces[fallback]
                .get_char_index('𠮷' as usize)
                .is_some()
        );
        assert!(terminal.rasterize('𠮷').bitmap.iter().any(|p| *p > 0));
    }
    #[test]
    fn reads_last_font_setting_without_accepting_comments() {
        assert_eq!(
            setting(
                "# font-family=wrong\nfont-family=first\nfont-family = \"FiraMono Nerd Font Mono\"\n",
                "font-family"
            ),
            Some("FiraMono Nerd Font Mono".into())
        );
        assert_eq!(
            setting("font-family=first\nfont-family=\n", "font-family"),
            None
        );
    }
}
