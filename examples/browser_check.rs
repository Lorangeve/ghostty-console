//! Run the installed browser in an isolated, offscreen PTY. No VT/device takeover.
use ghostty_console::{
    pty::Pty,
    renderer::{Renderer, save_ppm},
    terminal::Terminal,
};
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
struct BrowserProfile {
    root: PathBuf,
    env: Vec<(String, String)>,
}
impl Drop for BrowserProfile {
    fn drop(&mut self) {
        let _ = Command::new("timeout")
            .args(["5s", "terminal-browser", "shutdown"])
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if std::env::var_os("GHOSTTY_CONSOLE_KEEP_BROWSER_PROFILE").is_some() {
            eprintln!("Kept isolated browser diagnostics: {}", self.root.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args().nth(1).ok_or("Pass an output .ppm path")?;
    let root = std::env::temp_dir().join(format!("gc-browser-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let mut envs = Vec::new();
    for (name, dir) in [
        ("XDG_CONFIG_HOME", "c"),
        ("XDG_DATA_HOME", "d"),
        ("XDG_STATE_HOME", "s"),
        ("XDG_RUNTIME_DIR", "r"),
        ("TERMINAL_BROWSER_APPDATA", "p"),
        ("TERMINAL_BROWSER_INTEROP_DIR", "i"),
    ] {
        let path = root.join(dir);
        std::fs::create_dir_all(&path)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        envs.push((name.to_owned(), path.to_string_lossy().into_owned()));
    }
    envs.extend([
        ("TERM_PROGRAM".into(), "ghostty-console".into()),
        ("GHOSTTY_RESOURCES_DIR".into(), String::new()),
    ]);
    let profile = BrowserProfile { root, env: envs };
    let page = profile.root.join("page.html");
    std::fs::write(
        &page,
        r#"<html><body style="margin:0;background:#123456;color:white;font:32px sans-serif"><h1>con browser image test</h1><button style="position:absolute;left:20px;top:130px;width:180px;height:40px" onclick="document.querySelector('h1').textContent='mouse works';getSelection().selectAllChildren(document.querySelector('h1'))">Click me</button><script>let n=0;setInterval(()=>{document.body.style.background=(++n%2)?'#126634':'#123456'},500)</script></body></html>"#,
    )?;
    let mut renderer = Renderer::new()?;
    renderer.panel_visible = false;
    let (mut cols, mut rows) = (80u16, 24u16);
    let (cw, ch) = (renderer.cell_width(), renderer.cell_height());
    let mut terminal = Terminal::new(cols, rows)?;
    terminal.set_cell_size(cw, ch)?;
    let refs: Vec<(&str, &str)> = profile
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut pty = Pty::spawn_env(
        "/usr/bin/terminal-browser",
        &[
            "open",
            page.to_str().unwrap(),
            "--no-toolbar",
            "--no-frame",
            "--no-merge",
        ],
        cols,
        rows,
        &refs,
    )?;
    pty.resize(cols, rows, cw, ch)?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(18);
    let mut child_exited = false;
    let mut hashes = HashSet::new();
    let mut bytes = 0;
    let mut screenshot = false;
    let mut resized = false;
    let mut resize_confirmed = false;
    let mut click_time = None;
    let mut copy_sent = false;
    let mut copied = false;
    let mut capture = Vec::new();
    let mut buffer = [0; 65536];
    let mut next = Instant::now();
    while Instant::now() < deadline {
        for _ in 0..8 {
            match pty.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    bytes += n;
                    if capture.len() < 16384 {
                        capture.extend_from_slice(&buffer[..n.min(16384 - capture.len())]);
                    }
                    terminal.write(&buffer[..n]);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                Err(e) => return Err(e.into()),
            }
        }
        pty.send(&terminal.take_replies())?;
        if let Some(text) = terminal.take_clipboard() {
            copied = text.contains("mouse works");
        }
        if click_time.is_some_and(|t: Instant| t.elapsed() > Duration::from_millis(350))
            && !copy_sent
        {
            pty.send(&terminal.key(46, 1, 2, "c", 99)?)?;
            pty.send(&terminal.key(46, 0, 2, "c", 99)?)?;
            copy_sent = true;
        }
        if Instant::now() >= next {
            let frame = terminal.frame()?;
            for image in &frame.images {
                if resized
                    && image.dest_width == u32::from(cols) * cw as u32
                    && image.dest_height == u32::from(rows) * ch as u32
                {
                    resize_confirmed = true;
                }
                hashes.insert(image.pixels.iter().step_by(97).fold(0u64, |hash, p| {
                    hash.wrapping_mul(31).wrapping_add(u64::from(*p))
                }));
            }
            if !frame.images.is_empty() {
                let pixels = renderer.frame(
                    &frame,
                    usize::from(cols) * cw,
                    usize::from(rows) * ch,
                    None,
                    false,
                );
                save_ppm(
                    Path::new(&output),
                    usize::from(cols) * cw,
                    usize::from(rows) * ch,
                    &pixels,
                )?;
                screenshot = true;
            }
            next = Instant::now() + Duration::from_millis(150);
        }
        if !resized && hashes.len() >= 3 {
            cols += 8;
            rows += 3;
            terminal.resize(cols, rows)?;
            pty.resize(cols, rows, cw, ch)?;
            hashes.clear();
            resized = true;
        } else if resized && resize_confirmed && hashes.len() >= 2 && click_time.is_none() {
            for (action, button) in [(2, 0), (0, 1), (1, 1)] {
                pty.send(&terminal.mouse(action, button, 60., 150., 0)?)?;
            }
            click_time = Some(Instant::now());
        } else if copied {
            break;
        }
        if pty.exited()? {
            child_exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "browser elapsed={:.2}s child_exited={child_exited}",
        started.elapsed().as_secs_f32()
    );
    let _ = pty.send(b"\x03");
    drop(pty);
    println!(
        "browser bytes={bytes}, distinct image states={}, screenshot={screenshot}",
        hashes.len()
    );
    println!("browser resize confirmed={resize_confirmed}");
    println!("browser mouse click and clipboard confirmed={copied}");
    if !screenshot || hashes.len() < 2 || !resize_confirmed || !copied {
        eprintln!("initial output: {:?}", String::from_utf8_lossy(&capture));
        return Err("Browser did not deliver updating image frames".into());
    }
    Ok(())
}
