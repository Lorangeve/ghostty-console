//! Zellij-inspired mode badge and context-sensitive shortcut groups.
use crate::{
    renderer::{Renderer, rect},
    terminal::Frame,
};
const BG: u32 = 0x171b29;
const MUTED: u32 = 0x8b94aa;
const TEXT: u32 = 0xd9dfeb;
const KEY: u32 = 0xa6baff;
const CHIP: u32 = 0x293248;
impl Renderer {
    pub(crate) fn draw_status(
        &mut self,
        pixels: &mut [u32],
        width: usize,
        y: usize,
        frame: &Frame,
        enabled: bool,
    ) {
        let line = self.cell_height();
        rect(pixels, width, 0, y, width, line, BG);
        rect(pixels, width, 0, y, width, 1, 0x343d52);
        if self.login_screen {
            self.ui_text(
                pixels,
                width,
                8,
                y,
                "LOGIN · 用户名 / 密码    Ctrl+Alt+F1 返回桌面    Ctrl+Alt+Esc 重新登录",
                TEXT,
            );
            return;
        }
        let (mode, accent) = if self.login_screen {
            ("LOGIN", 0xe5c890)
        } else if self.buffers.window.buffer == crate::buffers::BufferId::Messages {
            ("*Messages* 只读", 0x81c8be)
        } else if self.help_visible {
            ("HELP", 0xc6a0f6)
        } else if self.selection.is_some() {
            ("SELECT", 0xe5c890)
        } else if enabled {
            ("RIME", 0xa6d189)
        } else if frame.mouse_tracking {
            ("APP", 0x81c8be)
        } else {
            ("EN", 0x8caaee)
        };
        let mode_width = self.ui_measure(mode) + 20;
        rounded(
            pixels,
            width,
            8,
            y + 3,
            mode_width,
            line.saturating_sub(6),
            accent,
        );
        self.ui_text(pixels, width, 18, y, mode, 0x141824);
        if self.buffers.window.buffer == crate::buffers::BufferId::Messages {
            let text = format!(
                "{} 条 · ↑↓/PgUp/PgDn 滚动 · ←→ 水平 · q/Esc 返回",
                self.buffers.messages.len()
            );
            self.ui_text(pixels, width, mode_width + 24, y, &text, TEXT);
            return;
        }
        let compact = width < 1100;
        let help_key = if compact { "C-A-H" } else { "Ctrl+Alt+H" };
        let help_width = self.hint_width(help_key, "帮助");
        let help_x = width.saturating_sub(help_width + 10);
        let limit = help_x.saturating_sub(18);
        let mut x = mode_width + 24;
        let caret = frame
            .caret
            .map(|(col, row)| format!(" {}:{}", row + 1, col + 1))
            .unwrap_or_default();
        let buffer_label = if compact {
            "*terminal*".to_string()
        } else {
            format!("*terminal*{caret} {}×{}", frame.cols, frame.rows)
        };
        let label_width = self.ui_measure(&buffer_label);
        if x + label_width + 16 < limit {
            self.ui_text(pixels, width, x, y, &buffer_label, TEXT);
            x += label_width + 20;
        }
        if x + 12 < limit {
            if compact {
                let hints = [
                    ("C-Spc", "中/英"),
                    ("C-A-N", "消息"),
                    ("C-A-I", "底栏"),
                    ("C-A-Esc", "退出"),
                    ("C-A-F1", "桌面"),
                    ("C-S-C/V", "复制/粘贴"),
                ];
                for (key, label) in hints {
                    let needed = self.hint_width(key, label);
                    if x + needed <= limit {
                        x = self.hint(pixels, width, x, y, key, label, KEY) + 8;
                    }
                }
            } else {
                for (prefix, items) in [
                    ("Ctrl", vec![("Space", "中/英")]),
                    (
                        "Ctrl+Alt",
                        vec![
                            ("I", "底栏"),
                            ("N", "消息"),
                            ("F1..12", "切屏"),
                            ("Esc", "退出"),
                        ],
                    ),
                    ("Ctrl+Shift", vec![("C/V", "复制/粘贴")]),
                ] {
                    let needed = self.ui_measure(prefix)
                        + 12
                        + items
                            .iter()
                            .map(|(k, l)| self.hint_width(k, l) + 12)
                            .sum::<usize>();
                    if x + needed > limit {
                        continue;
                    }
                    self.ui_text(pixels, width, x, y, prefix, MUTED);
                    x += self.ui_measure(prefix) + 12;
                    for (key, label) in items {
                        x = self.hint(pixels, width, x, y, key, label, KEY) + 12;
                    }
                    rect(
                        pixels,
                        width,
                        x + 2,
                        y + 7,
                        1,
                        line.saturating_sub(14),
                        0x3b4356,
                    );
                    x += 18;
                }
                let mouse = if self.selection.is_some() {
                    "右键复制并输入  中键粘贴"
                } else if frame.mouse_tracking {
                    "Shift+拖动 选择文字"
                } else {
                    "拖动选择  右键复制并输入"
                };
                let needed = self.ui_measure(mouse);
                if x + needed <= limit {
                    self.ui_text(pixels, width, x, y, mouse, MUTED);
                }
            }
        }
        if help_x >= mode_width + 28 {
            self.hint(pixels, width, help_x, y, help_key, "帮助", 0xc6a0f6);
        }
    }
    fn hint_width(&mut self, key: &str, label: &str) -> usize {
        self.ui_measure(key) + 12 + 6 + self.ui_measure(label)
    }
    #[allow(clippy::too_many_arguments)]
    fn hint(
        &mut self,
        pixels: &mut [u32],
        width: usize,
        x: usize,
        y: usize,
        key: &str,
        label: &str,
        color: u32,
    ) -> usize {
        let key_width = self.ui_measure(key) + 12;
        rounded(
            pixels,
            width,
            x,
            y + 4,
            key_width,
            self.cell_height().saturating_sub(8),
            CHIP,
        );
        self.ui_text(pixels, width, x + 6, y, key, color);
        self.ui_text(pixels, width, x + key_width + 6, y, label, TEXT)
    }
    pub(crate) fn draw_help(&mut self, pixels: &mut [u32], width: usize, height: usize) {
        if !self.help_visible {
            return;
        }
        for p in pixels.iter_mut() {
            *p = (*p & 0xfcfcfc) >> 2;
        }
        let line = self.cell_height();
        let panel_width = width.saturating_sub(24).min(960);
        let panel_height = (line * 14 + 16).min(height.saturating_sub(20));
        let x = (width - panel_width) / 2;
        let y = (height - panel_height) / 2;
        rounded(
            pixels,
            width,
            x + 3,
            y + 4,
            panel_width,
            panel_height,
            0x090c14,
        );
        rounded(pixels, width, x, y, panel_width, panel_height, 0x202739);
        rect(pixels, width, x, y, panel_width, 2, 0x8caaee);
        self.ui_text(pixels, width, x + 18, y + 8, "CON  /  快捷键", 0xc6a0f6);
        let compact = panel_width < 740;
        let rows = [
            ("Ctrl+Space", "C-Spc", "切换英文 / Rime"),
            ("Ctrl+Alt+I", "C-A-I", "显示 / 隐藏底栏"),
            ("Ctrl+Alt+F1..F12", "C-A-F1..12", "切换虚拟控制台"),
            ("Ctrl+Alt+Esc", "C-A-Esc", "退出 con"),
            ("Ctrl+Shift+C / V", "C-S-C/V", "复制 / 粘贴"),
            ("鼠标左键拖动", "左键拖动", "选择文字"),
            ("鼠标右键 / 中键", "右键 / 中键", "复制并输入 / 粘贴"),
            ("Shift+拖动", "Shift+拖动", "应用内强制选择终端文字"),
            ("Ctrl+Alt+H", "C-A-H", "显示 / 隐藏帮助"),
            ("Ctrl+Alt+M", "C-A-M", "候选显示：行内 / 浮窗"),
            ("Ctrl+Alt+N", "C-A-N", "查看 / 返回 *Messages* 消息历史"),
        ];
        let key_width = if compact { 145 } else { 300 };
        for (i, (full, short, label)) in rows.into_iter().enumerate() {
            let row_y = y + 8 + line * (i + 2);
            if row_y + line > y + panel_height - line {
                break;
            }
            self.ui_text(
                pixels,
                width,
                x + 18,
                row_y,
                if compact { short } else { full },
                KEY,
            );
            let available = x + panel_width - 18;
            if x + key_width + self.ui_measure(label) <= available {
                self.ui_text(pixels, width, x + key_width, row_y, label, TEXT);
            }
        }
        let footer = if compact {
            "Esc 关闭 · C=Ctrl A=Alt S=Shift"
        } else {
            "Esc 或任意按键关闭    C = Ctrl · A = Alt · S = Shift"
        };
        if self.ui_measure(footer) + 36 <= panel_width {
            self.ui_text(
                pixels,
                width,
                x + 18,
                y + panel_height - line - 6,
                footer,
                MUTED,
            );
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn rounded(
    pixels: &mut [u32],
    width: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: u32,
) {
    if w < 4 || h < 4 {
        rect(pixels, width, x, y, w, h, color);
        return;
    }
    rect(pixels, width, x + 2, y, w - 4, h, color);
    rect(pixels, width, x, y + 2, w, h - 4, color);
    rect(pixels, width, x + 1, y + 1, w - 2, h - 2, color);
}
