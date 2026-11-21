//! Content survives window changes. Echo messages expire; *Messages* history does not.
use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Debug)]
pub struct Message {
    pub source: String,
    pub text: String,
    pub seconds: u64,
}
impl Message {
    pub fn new(source: &str, text: &str) -> Self {
        let clean = |s: &str| {
            s.chars()
                .filter(|c| !c.is_control() || *c == '\n')
                .take(8192)
                .collect()
        };
        Self {
            source: source
                .chars()
                .filter(|c| !c.is_control())
                .take(128)
                .collect(),
            text: clean(text),
            seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
    pub fn line(&self) -> String {
        let mut local: libc::tm = unsafe { std::mem::zeroed() };
        let timestamp = self.seconds as libc::time_t;
        unsafe {
            libc::localtime_r(&timestamp, &mut local);
        }
        format!(
            "[{:02}:{:02}:{:02}] {}: {}",
            local.tm_hour, local.tm_min, local.tm_sec, self.source, self.text
        )
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BufferId {
    Terminal,
    Messages,
}
/// A window selects a buffer and retains its independent distance from the tail.
pub struct Window {
    pub buffer: BufferId,
    pub scroll: usize,
    pub horizontal: usize,
}
pub struct Buffers {
    pub window: Window,
    pub messages: VecDeque<Message>,
    echo: Option<(Message, Instant)>,
    limit: usize,
}
impl Default for Buffers {
    fn default() -> Self {
        Self {
            window: Window {
                buffer: BufferId::Terminal,
                scroll: 0,
                horizontal: 0,
            },
            messages: VecDeque::new(),
            echo: None,
            limit: 1000,
        }
    }
}
impl Buffers {
    pub fn push(&mut self, message: Message) {
        if self.window.scroll > 0 {
            self.window.scroll += message.text.lines().count().max(1);
        }
        self.echo = Some((message.clone(), Instant::now() + Duration::from_secs(5)));
        self.messages.push_back(message);
        while self.messages.len() > self.limit {
            self.messages.pop_front();
        }
        self.window.scroll = self.window.scroll.min(self.line_count().saturating_sub(1));
    }
    pub fn line_count(&self) -> usize {
        self.messages
            .iter()
            .map(|m| m.text.lines().count().max(1))
            .sum()
    }
    pub fn lines(&self) -> Vec<String> {
        self.messages
            .iter()
            .flat_map(|m| m.line().lines().map(str::to_owned).collect::<Vec<_>>())
            .collect()
    }
    pub fn echo(&self) -> Option<&Message> {
        self.echo.as_ref().map(|(m, _)| m)
    }
    pub fn tick(&mut self) -> bool {
        if self
            .echo
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until)
        {
            self.echo = None;
            true
        } else {
            false
        }
    }
    pub fn toggle_messages(&mut self) {
        self.window.buffer = if self.window.buffer == BufferId::Messages {
            BufferId::Terminal
        } else {
            BufferId::Messages
        };
    }
    pub fn scroll(&mut self, amount: isize) {
        self.window.scroll = self
            .window
            .scroll
            .saturating_add_signed(amount)
            .min(self.line_count().saturating_sub(1));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiline_history_preserves_lines_and_scroll_anchor() {
        let mut b = Buffers::default();
        b.push(Message::new("build", "title\nbody"));
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.lines()[1], "body");
        b.scroll(1);
        b.push(Message::new("app", "next\nline"));
        assert_eq!(b.window.scroll, 3);
        b.scroll(-100);
        assert_eq!(b.window.scroll, 0);
    }
    #[test]
    fn history_survives_echo_and_window_changes() {
        let mut b = Buffers {
            limit: 3,
            ..Buffers::default()
        };
        for i in 0..4 {
            b.push(Message::new("test", &i.to_string()));
        }
        b.toggle_messages();
        b.scroll(1);
        b.push(Message::new("test", "next"));
        assert_eq!(b.window.scroll, 2);
        assert_eq!(b.messages.len(), 3);
        b.echo.as_mut().unwrap().1 = Instant::now();
        assert!(b.tick());
        assert!(b.echo().is_none());
        b.toggle_messages();
        b.toggle_messages();
        assert_eq!(b.window.scroll, 2);
        assert_eq!(b.messages.back().unwrap().text, "next");
    }
}
