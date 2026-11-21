use ghostty_console::{
    config, console,
    renderer::{Renderer, save_ppm},
    settings,
    terminal::Terminal,
};
use std::{env, path::Path};
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut explicit = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--config-file" {
            args.remove(index);
            if index >= args.len() {
                return Err("--config-file requires a path".into());
            }
            explicit = Some(std::path::PathBuf::from(args.remove(index)));
        } else if let Some(value) = args[index].strip_prefix("--config-file=") {
            explicit = Some(std::path::PathBuf::from(value));
            args.remove(index);
        } else {
            index += 1;
        }
    }
    // Lifecycle helpers do not consume configuration. A broken renderer config
    // must never disable emergency session cleanup or the authenticated gate.
    match args.first().map(String::as_str) {
        Some("login-cleanup") => return ghostty_console::session::cleanup_record(),
        Some("login-shell") => return ghostty_console::login_gate::shell(),
        _ => (),
    }
    let settings = settings::initialize(explicit.as_deref())?;
    match args.first().map(String::as_str) {
        Some("+show-config") => {
            println!("{}", settings.show());
            Ok(())
        }
        Some("+validate-config") => {
            println!(
                "Configuration valid: {}",
                settings
                    .source
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "built-in defaults".into())
            );
            Ok(())
        }
        Some("login-service") => ghostty_console::login_service::launch(
            args.get(1)
                .ok_or("login-service requires a renderer user")?,
        ),
        Some("login-worker") => console::login_worker(
            args.get(1).ok_or("login-worker requires a renderer user")?,
            match args.get(2).map(String::as_str) {
                None | Some("greeter") => true,
                Some("user") => false,
                _ => return Err("Invalid login-worker role".into()),
            },
        ),
        Some("con") => console::launch(&args[1..]),
        Some("con-worker") => console::worker(&args[1..]),
        Some("rime-probe") => {
            let mut rime = config::open_rime(args.get(1).map(String::as_str))?;
            let keys = args.get(2).map(String::as_str).unwrap_or("nihao");
            if !keys.is_ascii() {
                return Err("Probe KEYS must be ASCII".into());
            }
            let mut commit = String::new();
            for key in keys.bytes().chain(std::iter::once(b' ')) {
                let update = rime.process_key(key.into(), 0);
                println!(
                    "key={:?} handled={} preedit={:?}",
                    char::from(key),
                    update.handled,
                    update.preedit
                );
                for candidate in &update.candidates {
                    println!(
                        "  {} {} {}",
                        candidate.label, candidate.text, candidate.comment
                    );
                }
                commit.push_str(&update.commit);
            }
            println!("commit={commit:?}");
            Ok(())
        }
        Some("preview") => {
            let output = args.get(1).map(String::as_str).unwrap_or("preview.ppm");
            let mut terminal = Terminal::new(90, 22)?;
            terminal.write("\x1b[2J\x1b[H\x1b[1;36mghostty-console\x1b[0m  Rust + libghostty-vt + Rime\r\n\r\n中文显示：你好，世界。\r\n\x1b[31mRED \x1b[32mGREEN \x1b[34mBLUE\x1b[0m\r\n\r\nNerd Font: \u{e0b0} \u{f120} \u{f07b} \u{f126}\r\n\r\nfish > ".as_bytes());
            let mut rime = config::open_rime(Some("wubi98_ci"))?;
            let mut ime = None;
            for key in b"wqvb" {
                ime = Some(rime.process_key((*key).into(), 0));
            }
            let frame = terminal.frame()?;
            let mut renderer = Renderer::new()?;
            let (w, h) = (90 * renderer.cell_width(), 25 * renderer.cell_height());
            let pixels = renderer.frame(&frame, w, h, ime.as_ref(), true);
            save_ppm(Path::new(output), w, h, &pixels)?;
            println!("Wrote {output} ({w}x{h})");
            Ok(())
        }
        _ => {
            println!(
                "ghostty-console [--config-file PATH]\n  +show-config\n  +validate-config\n  con [--check] [--timeout SECONDS] [--device /dev/dri/cardN]\n  preview [FILE.ppm]\n  rime-probe [SCHEMA] [KEYS]\n\ncon requires a native getty VT (not kmscon or a desktop terminal).\nCtrl+Space: Rime | Ctrl+Alt+Fn: switch VT | Ctrl+Alt+Esc: exit | Ctrl+Alt+I: panel | Ctrl+Alt+H: shortcuts | Ctrl+Alt+M: IME layout | Ctrl+Alt+N: *Messages*\nMouse: left-drag selects, right-click copies and pastes, middle-click pastes (Shift overrides app mouse tracking).\nDefault con timeout: 120 seconds (0 disables)."
            );
            Ok(())
        }
    }
}
