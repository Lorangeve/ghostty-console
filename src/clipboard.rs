//! Clipboard IPC runs outside the seat/render loop, with bounded queues and helper deadlines.
use std::{
    env,
    io::Write,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
};
enum Request {
    Copy(String),
    Paste(String, u64),
}
struct Backend {
    display: String,
    runtime: Option<String>,
    copy: String,
    paste: String,
}
impl Backend {
    fn detect() -> Option<Self> {
        let display = env::var("WAYLAND_DISPLAY").ok().or_else(|| {
            let result = Command::new("timeout")
                .args(["2s", "systemctl", "--user", "show-environment"])
                .output()
                .ok()?;
            String::from_utf8(result.stdout)
                .ok()?
                .lines()
                .find_map(|line| line.strip_prefix("WAYLAND_DISPLAY=").map(str::to_owned))
        })?;
        Some(Self {
            display,
            runtime: env::var("XDG_RUNTIME_DIR").ok(),
            copy: "wl-copy".into(),
            paste: "wl-paste".into(),
        })
    }
    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new("timeout");
        cmd.args(["2s", program])
            .env("WAYLAND_DISPLAY", &self.display)
            .stderr(Stdio::null());
        if let Some(runtime) = &self.runtime {
            cmd.env("XDG_RUNTIME_DIR", runtime);
        }
        cmd
    }
    fn copy(&self, text: &str) -> bool {
        let mut cmd = self.command(&self.copy);
        cmd.args(["--type", "text/plain;charset=utf-8"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null());
        let Ok(mut child) = cmd.spawn() else {
            return false;
        };
        let written = child
            .stdin
            .take()
            .is_some_and(|mut input| input.write_all(text.as_bytes()).is_ok());
        child.wait().is_ok_and(|status| status.success()) && written
    }
    fn paste(&self) -> Option<String> {
        let output = self
            .command(&self.paste)
            .args(["--no-newline", "--type", "text"])
            .output()
            .ok()?;
        if !output.status.success() || output.stdout.len() > 2 * 1024 * 1024 {
            return None;
        }
        String::from_utf8(output.stdout).ok()
    }
}
pub struct PasteReply {
    pub text: String,
    generation: u64,
}
pub struct Clipboard {
    tx: SyncSender<Request>,
    pub replies: Receiver<PasteReply>,
    generation: u64,
    pub messages: Receiver<crate::buffers::Message>,
    local: String,
}
impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}
impl Clipboard {
    pub fn local_only() -> Self {
        Self::spawn(|| None)
    }
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn is_current(&self, reply: &PasteReply) -> bool {
        reply.generation == self.generation
    }
    pub fn new() -> Self {
        Self::spawn(Backend::detect)
    }
    fn spawn(detect: impl FnOnce() -> Option<Backend> + Send + 'static) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Request>(8);
        let (reply_tx, replies) = mpsc::channel();
        let (message_tx, messages) = mpsc::sync_channel(16);
        std::thread::spawn(move || {
            let backend = detect();
            while let Ok(request) = rx.recv() {
                match request {
                    Request::Copy(text) => {
                        if backend.as_ref().is_some_and(|b| b.copy(&text)) {
                            let _ = message_tx.try_send(crate::buffers::Message::new(
                                "剪贴板",
                                "已复制到桌面剪贴板",
                            ));
                        } else {
                            let _ = message_tx.try_send(crate::buffers::Message::new(
                                "剪贴板",
                                "桌面剪贴板不可用，已保存在 con 内",
                            ));
                        }
                    }
                    Request::Paste(fallback, generation) => {
                        let text = backend
                            .as_ref()
                            .and_then(Backend::paste)
                            .unwrap_or(fallback);
                        let _ = reply_tx.send(PasteReply { text, generation });
                    }
                }
            }
        });
        Self {
            tx,
            replies,
            messages,
            local: String::new(),
            generation: 0,
        }
    }
    pub fn copy(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.local = text.clone();
        let _ = self.tx.try_send(Request::Copy(text));
    }
    pub fn paste(&self) {
        let _ = self
            .tx
            .try_send(Request::Paste(self.local.clone(), self.generation));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_paste_cannot_cross_a_privacy_transition() {
        let mut clipboard = Clipboard::local_only();
        clipboard.copy("stale".into());
        clipboard.paste();
        clipboard.invalidate();
        let reply = clipboard
            .replies
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert!(!clipboard.is_current(&reply));
        clipboard.paste();
        let reply = clipboard
            .replies
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert!(clipboard.is_current(&reply));
    }
    #[test]
    fn local_clipboard_works_without_desktop() {
        let mut clipboard = Clipboard::spawn(|| None);
        clipboard.copy("你好".into());
        clipboard.paste();
        assert_eq!(
            clipboard
                .replies
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .text,
            "你好"
        );
    }
    #[test]
    fn helper_round_trip_preserves_utf8_and_newlines() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("gc-clipboard-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let copy = root.join("copy");
        let paste = root.join("paste");
        std::fs::write(&copy, "#!/bin/sh\ncat > \"${0%/*}/clipboard\"\n").unwrap();
        std::fs::write(&paste, "#!/bin/sh\ncat \"${0%/*}/clipboard\"\n").unwrap();
        for path in [&copy, &paste] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let backend = Backend {
            display: "mock".into(),
            runtime: None,
            copy: copy.to_string_lossy().into_owned(),
            paste: paste.to_string_lossy().into_owned(),
        };
        let mut clipboard = Clipboard::spawn(move || Some(backend));
        clipboard.copy("你好\nline 2".into());
        clipboard.paste();
        assert_eq!(
            clipboard
                .replies
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
                .text,
            "你好\nline 2"
        );
        drop(clipboard);
        std::fs::remove_dir_all(root).unwrap();
    }
}
