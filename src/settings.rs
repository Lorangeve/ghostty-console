//! Ghostty-style key/value configuration. Search stops at the first existing file.
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};
static SETTINGS: OnceLock<Result<Settings, String>> = OnceLock::new();
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImeDisplay {
    Inline,
    Popup,
}
impl ImeDisplay {
    pub fn toggle(&mut self) {
        *self = match self {
            Self::Inline => Self::Popup,
            Self::Popup => Self::Inline,
        };
    }
}
#[derive(Clone, Debug)]
pub struct Settings {
    pub source: Option<PathBuf>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub ime_font_family: Option<String>,
    pub ime_font_size: Option<f32>,
    pub command: Vec<String>,
    pub timeout: u64,
    pub drm_device: Option<PathBuf>,
    pub status_bar_visible: bool,
    pub desktop_notifications: bool,
    pub ime_enabled: bool,
    pub ime_display: ImeDisplay,
    pub keyboard_layout: String,
    pub rime_schema: Option<String>,
    pub rime_source_dir: Option<PathBuf>,
    pub rime_user_dir: Option<PathBuf>,
    pub rime_shared_dir: PathBuf,
    pub background: Option<u32>,
    pub foreground: Option<u32>,
    pub cursor_color: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            source: None,
            font_family: None,
            font_size: None,
            ime_font_family: None,
            ime_font_size: None,
            command: vec!["/usr/bin/fish".into(), "--interactive".into()],
            timeout: 120,
            drm_device: None,
            status_bar_visible: true,
            desktop_notifications: true,
            ime_enabled: false,
            ime_display: ImeDisplay::Inline,
            keyboard_layout: "us".into(),
            rime_schema: None,
            rime_source_dir: None,
            rime_user_dir: None,
            rime_shared_dir: "/usr/share/rime-data".into(),
            background: None,
            foreground: None,
            cursor_color: 0xe5cf80,
        }
    }
}
pub fn config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}
pub fn candidates() -> Result<Vec<PathBuf>, String> {
    let mut paths = vec![
        env::current_dir()
            .map_err(|e| e.to_string())?
            .join("ghostty-console.conf"),
    ];
    if let Some(home) = config_home() {
        paths.push(home.join("ghostty-console/config"));
    }
    paths.push("/etc/ghostty-console/config".into());
    Ok(paths)
}
pub fn initialize(explicit: Option<&Path>) -> Result<&'static Settings, String> {
    let configured = explicit.map(PathBuf::from).or_else(|| {
        env::var_os("GHOSTTY_CONSOLE_CONFIG")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    });
    SETTINGS
        .get_or_init(|| match configured {
            Some(path) => Settings::from_file(&path),
            None => Settings::search(&candidates()?),
        })
        .as_ref()
        .map_err(Clone::clone)
}
pub fn get() -> Result<&'static Settings, String> {
    initialize(None)
}
impl Settings {
    pub fn search(paths: &[PathBuf]) -> Result<Self, String> {
        for path in paths {
            match fs::read_to_string(path) {
                Ok(text) => return Self::parse(&text, path),
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Ok(Self::default())
    }
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text, path)
    }
    pub fn parse(text: &str, path: &Path) -> Result<Self, String> {
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            env::current_dir().map_err(|e| e.to_string())?.join(path)
        };
        let base = absolute.parent().unwrap_or(Path::new("/"));
        let mut config = Self {
            source: Some(absolute.clone()),
            ..Self::default()
        };
        let defaults = Self::default();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!(
                    "{}:{}: expected key = value",
                    path.display(),
                    index + 1
                ));
            };
            let key = key.trim();
            let value = value.trim();
            let result = (|| -> Result<(), String> {
                let value = if key == "command" {
                    value.to_owned()
                } else {
                    unquote(value)?
                };
                let value = value.as_str();
                match key {
                    "font-family" => config.font_family = optional(value),
                    "font-size" => config.font_size = points(value)?,
                    "ime-font-family" => config.ime_font_family = optional(value),
                    "ime-font-size" => config.ime_font_size = points(value)?,
                    "command" => {
                        config.command = if value.is_empty() {
                            defaults.command.clone()
                        } else {
                            shlex::split(value)
                                .filter(|v| !v.is_empty())
                                .ok_or("Invalid command quoting")?
                        };
                        if config.command[0].is_empty() {
                            return Err("Command executable is empty".into());
                        }
                        if config.command[0].starts_with("~/")
                            || config.command[0].contains('/')
                                && !Path::new(&config.command[0]).is_absolute()
                        {
                            config.command[0] = resolve(&config.command[0], base)?
                                .to_string_lossy()
                                .into_owned();
                        }
                    }
                    "timeout" => {
                        config.timeout = if value.is_empty() {
                            defaults.timeout
                        } else {
                            value
                                .parse()
                                .map_err(|_| "timeout must be a nonnegative integer")?
                        }
                    }
                    "drm-device" => config.drm_device = optional_path(value, base)?,
                    "status-bar-visible" => {
                        config.status_bar_visible = boolean(value, defaults.status_bar_visible)?
                    }
                    "desktop-notifications" => {
                        config.desktop_notifications =
                            boolean(value, defaults.desktop_notifications)?
                    }
                    "ime-display" => {
                        config.ime_display = match value {
                            "" | "inline" => ImeDisplay::Inline,
                            "popup" => ImeDisplay::Popup,
                            _ => return Err("ime-display must be inline or popup".into()),
                        }
                    }
                    "ime-enabled" => config.ime_enabled = boolean(value, defaults.ime_enabled)?,
                    "keyboard-layout" => {
                        config.keyboard_layout =
                            optional(value).unwrap_or_else(|| defaults.keyboard_layout.clone())
                    }
                    "rime-schema" => config.rime_schema = optional(value),
                    "rime-source-dir" => config.rime_source_dir = optional_path(value, base)?,
                    "rime-user-dir" => config.rime_user_dir = optional_path(value, base)?,
                    "rime-shared-dir" => {
                        config.rime_shared_dir = optional_path(value, base)?
                            .unwrap_or_else(|| defaults.rime_shared_dir.clone())
                    }
                    "background" => config.background = color(value)?,
                    "foreground" => config.foreground = color(value)?,
                    "cursor-color" => {
                        config.cursor_color = color(value)?.unwrap_or(defaults.cursor_color)
                    }
                    _ => return Err(format!("Unknown configuration key: {key}")),
                }
                Ok(())
            })();
            if let Err(error) = result {
                return Err(format!("{}:{}: {error}", path.display(), index + 1));
            }
        }
        Ok(config)
    }
    pub fn show(&self) -> String {
        let mut lines = vec![format!(
            "# config: {}",
            self.source
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "built-in defaults".into())
        )];
        for (key, value) in [
            ("font-family", self.font_family.clone().unwrap_or_default()),
            (
                "font-size",
                self.font_size.map(|v| v.to_string()).unwrap_or_default(),
            ),
            (
                "ime-font-family",
                self.ime_font_family.clone().unwrap_or_default(),
            ),
            (
                "ime-font-size",
                self.ime_font_size
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                "command",
                shlex::try_join(self.command.iter().map(String::as_str)).unwrap_or_default(),
            ),
            ("timeout", self.timeout.to_string()),
            ("drm-device", display_path(&self.drm_device)),
            ("status-bar-visible", self.status_bar_visible.to_string()),
            (
                "desktop-notifications",
                self.desktop_notifications.to_string(),
            ),
            ("ime-enabled", self.ime_enabled.to_string()),
            (
                "ime-display",
                match self.ime_display {
                    ImeDisplay::Inline => "inline",
                    ImeDisplay::Popup => "popup",
                }
                .into(),
            ),
            ("keyboard-layout", self.keyboard_layout.clone()),
            ("rime-schema", self.rime_schema.clone().unwrap_or_default()),
            ("rime-source-dir", display_path(&self.rime_source_dir)),
            ("rime-user-dir", display_path(&self.rime_user_dir)),
            (
                "rime-shared-dir",
                self.rime_shared_dir.display().to_string(),
            ),
            (
                "background",
                self.background
                    .map(|v| format!("#{v:06x}"))
                    .unwrap_or_default(),
            ),
            (
                "foreground",
                self.foreground
                    .map(|v| format!("#{v:06x}"))
                    .unwrap_or_default(),
            ),
            ("cursor-color", format!("#{:06x}", self.cursor_color)),
        ] {
            lines.push(format!("{key} = {value}"));
        }
        lines.join("\n")
    }
}
fn optional(v: &str) -> Option<String> {
    if v.is_empty() { None } else { Some(v.into()) }
}
fn display_path(p: &Option<PathBuf>) -> String {
    p.as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}
fn unquote(v: &str) -> Result<String, String> {
    if v.starts_with('"') || v.starts_with('\'') {
        let first = v.as_bytes()[0];
        if v.len() < 2 || v.as_bytes().last() != Some(&first) {
            return Err("Unclosed quoted value".into());
        }
        Ok(v[1..v.len() - 1].into())
    } else {
        Ok(v.into())
    }
}
fn boolean(v: &str, default: bool) -> Result<bool, String> {
    match v {
        "" => Ok(default),
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("Expected true or false".into()),
    }
}
fn points(v: &str) -> Result<Option<f32>, String> {
    if v.is_empty() {
        return Ok(None);
    }
    let n = v
        .parse::<f32>()
        .map_err(|_| "Font size must be numeric (points)")?;
    if !n.is_finite() || !(6. ..=72.).contains(&n) {
        return Err("Font size must be between 6 and 72 points".into());
    }
    Ok(Some(n))
}
fn color(v: &str) -> Result<Option<u32>, String> {
    if v.is_empty() {
        return Ok(None);
    }
    let hex = v.strip_prefix('#').unwrap_or(v);
    if hex.len() != 6 {
        return Err("Color must be #RRGGBB".into());
    }
    u32::from_str_radix(hex, 16)
        .map(Some)
        .map_err(|_| "Color must be #RRGGBB".into())
}
fn resolve(v: &str, base: &Path) -> Result<PathBuf, String> {
    if v == "~" || v.starts_with("~/") {
        let home = env::var_os("HOME").ok_or("HOME is missing")?;
        return Ok(PathBuf::from(home).join(v.strip_prefix("~/").unwrap_or("")));
    }
    let path = PathBuf::from(v);
    Ok(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}
fn optional_path(v: &str, base: &Path) -> Result<Option<PathBuf>, String> {
    if v.is_empty() {
        Ok(None)
    } else {
        resolve(v, base).map(Some)
    }
}
