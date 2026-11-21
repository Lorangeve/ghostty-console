use ghostty_console::settings::Settings;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "gc-settings-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn parses_quotes_colors_resets_and_argv_without_shell_execution() {
    let cfg=Settings::parse("# comment\nfont-family = \"FiraMono Nerd Font Mono\"\nfont-size = 14\nfont-size =\ntimeout = 42\nbackground = #123abc\ncommand = \"/bin/a shell\" --interactive 'two words'\nstatus-bar-visible = false\nime-display = popup\nrime-source-dir = ./data\n",std::path::Path::new("/tmp/settings/config")).unwrap();
    assert_eq!(cfg.font_family.as_deref(), Some("FiraMono Nerd Font Mono"));
    assert_eq!(cfg.font_size, None);
    assert_eq!(cfg.background, Some(0x123abc));
    assert_eq!(cfg.timeout, 42);
    assert!(!cfg.status_bar_visible);
    assert_eq!(
        cfg.ime_display,
        ghostty_console::settings::ImeDisplay::Popup
    );
    assert_eq!(
        cfg.command,
        vec!["/bin/a shell", "--interactive", "two words"]
    );
    assert_eq!(
        cfg.rime_source_dir,
        Some(PathBuf::from("/tmp/settings/./data"))
    );
}
#[test]
fn first_existing_file_wins_even_if_empty_or_invalid() {
    let root = Scratch::new();
    let local = root.0.join("local");
    let xdg = root.0.join("xdg");
    let system = root.0.join("system");
    let paths = vec![local.clone(), xdg.clone(), system.clone()];
    fs::write(&system, "timeout = 30").unwrap();
    assert_eq!(Settings::search(&paths).unwrap().timeout, 30);
    fs::write(&xdg, "timeout = 20").unwrap();
    assert_eq!(Settings::search(&paths).unwrap().timeout, 20);
    fs::write(&local, "timeout = 10").unwrap();
    assert_eq!(Settings::search(&paths).unwrap().timeout, 10);
    fs::write(&local, "").unwrap();
    assert_eq!(Settings::search(&paths).unwrap().timeout, 120);
    fs::write(&local, "font-size = NaN").unwrap();
    assert!(Settings::search(&paths).unwrap_err().contains("local:1:"));
}
#[test]
fn bad_options_have_line_numbers_and_do_not_silently_fall_back() {
    for input in [
        "timeout = -1",
        "background = #xyzxyz",
        "font-size = inf",
        "command = '",
        "ime-display = yes",
        "not-an-option = 1",
    ] {
        let error = Settings::parse(
            &format!("# first\n{input}"),
            std::path::Path::new("/tmp/config"),
        )
        .unwrap_err();
        assert!(error.contains("/tmp/config:2:"), "{error}");
    }
}
#[test]
fn cli_discovers_cwd_then_xdg_and_explicit_override() {
    let root = Scratch::new();
    let cwd = root.0.join("cwd");
    let xdg = root.0.join("xdg");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(xdg.join("ghostty-console")).unwrap();
    fs::write(xdg.join("ghostty-console/config"), "timeout = 23").unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ghostty-console"))
            .args(args)
            .current_dir(&cwd)
            .env("XDG_CONFIG_HOME", &xdg)
            .env_remove("GHOSTTY_CONSOLE_CONFIG")
            .output()
            .unwrap()
    };
    let result = run(&["+show-config"]);
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("timeout = 23"));
    fs::write(cwd.join("ghostty-console.conf"), "timeout = 11").unwrap();
    let result = run(&["+show-config"]);
    assert!(String::from_utf8_lossy(&result.stdout).contains("timeout = 11"));
    let explicit = root.0.join("explicit.conf");
    fs::write(&explicit, "timeout = 37").unwrap();
    let result = run(&["+show-config", "--config-file", explicit.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&result.stdout).contains("timeout = 37"));
}
