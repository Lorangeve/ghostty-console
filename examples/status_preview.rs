//! Render status-bar states without opening a seat, shell or Rime engine.
use ghostty_console::{
    interaction::Selection,
    renderer::{Renderer, save_ppm},
    rime::{Candidate, Update},
    terminal::Terminal,
};
use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(std::env::args().nth(1).ok_or("Pass an output directory")?);
    std::fs::create_dir_all(&output)?;
    let mut renderer = Renderer::new()?;
    let composing = Update {
        handled: true,
        commit: String::new(),
        preedit: "wqvb".into(),
        cursor: 4,
        selection: 0..4,
        page: 0,
        highlighted: 0,
        last_page: true,
        candidates: vec![
            Candidate {
                text: "你好".into(),
                comment: "〔 󰄥󰄾󰄟󰂍 〕".into(),
                label: "1".into(),
            },
            Candidate {
                text: "您好".into(),
                comment: "〔 󰄥󰄾󰄟󰂍 〕".into(),
                label: "2".into(),
            },
        ],
    };
    for (name, cols, enabled, ime, help, select, mouse) in [
        ("messages", 90, false, None, false, false, false),
        ("notify", 90, false, None, false, false, false),
        ("normal", 174, false, None, false, false, false),
        ("rime", 174, true, Some(&composing), false, false, false),
        ("app", 174, false, None, false, false, true),
        ("selection", 90, false, None, false, true, false),
        ("normal90", 90, false, None, false, false, false),
        ("rime90", 90, true, Some(&composing), false, false, false),
        ("popup", 90, true, Some(&composing), false, false, false),
        (
            "popup-bottom",
            90,
            true,
            Some(&composing),
            false,
            false,
            false,
        ),
        (
            "popup-hidden",
            90,
            true,
            Some(&composing),
            false,
            false,
            false,
        ),
        ("cursor", 90, false, None, false, false, false),
        ("compact", 80, false, None, false, false, false),
        ("narrow", 40, false, None, false, false, false),
        ("help", 90, false, None, true, false, false),
    ] {
        renderer.buffers = ghostty_console::buffers::Buffers::default();
        if name == "messages" || name == "notify" {
            for (source, text) in [
                ("con", "终端已就绪"),
                ("构建", "cargo build 完成"),
                ("notify-send", "你好，来自原生 TTY 的通知"),
                ("剪贴板", "已保存选中文字"),
            ] {
                renderer
                    .buffers
                    .push(ghostty_console::buffers::Message::new(source, text));
            }
            if name == "messages" {
                renderer.buffers.toggle_messages();
            }
        }
        renderer.help_visible = help;
        renderer.ime_display = if name.starts_with("popup") {
            ghostty_console::settings::ImeDisplay::Popup
        } else {
            ghostty_console::settings::ImeDisplay::Inline
        };
        renderer.pointer = if name == "cursor" {
            Some((180, 100))
        } else {
            None
        };
        renderer.selection = if select {
            Some(Selection {
                anchor: (0, 1),
                head: (18, 1),
            })
        } else {
            None
        };
        let reserved = renderer.panel_rows(enabled, ime);
        let total_rows = if help { 22usize } else { 10 };
        let mut terminal = Terminal::new(cols, (total_rows - reserved) as u16)?;
        terminal.write("\x1b[36mghostty-console\x1b[0m\r\n~/mycoding > cargo run --release -- con\r\n中文显示 · FiraMono + Rime 98WB-2\r\n".as_bytes());
        match name {
            "popup" => terminal.write(b"\x1b[4;25H"),
            "popup-bottom" => terminal.write(b"\x1b[9;85H"),
            "popup-hidden" => terminal.write(b"\x1b[6;35H\x1b[?25l"),
            _ => {}
        }
        let mut frame = terminal.frame()?;
        frame.mouse_tracking = mouse;
        let width = usize::from(cols) * renderer.cell_width();
        let height = (usize::from(frame.rows) + reserved) * renderer.cell_height();
        let pixels = renderer.frame(&frame, width, height, ime, enabled);
        save_ppm(&output.join(format!("{name}.ppm")), width, height, &pixels)?;
        println!("{name}: {width}x{height}, panel rows={reserved}");
    }
    Ok(())
}
