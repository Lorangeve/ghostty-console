use crate::terminal::Frame;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (usize, usize),
    pub head: (usize, usize),
}
impl Selection {
    pub fn range(&self, cols: usize) -> (usize, usize) {
        let a = self.anchor.1 * cols + self.anchor.0;
        let b = self.head.1 * cols + self.head.0;
        (a.min(b), a.max(b))
    }
    pub fn contains(&self, x: usize, y: usize, cols: usize) -> bool {
        let (a, b) = self.range(cols);
        (a..=b).contains(&(y * cols + x))
    }
    pub fn text(&self, frame: &Frame) -> String {
        let cols = usize::from(frame.cols);
        let (mut a, b) = self.range(cols);
        let b = b.min(frame.cells.len().saturating_sub(1));
        if a >= frame.cells.len() {
            return String::new();
        }
        if frame.cells[a].width == 0 && a > 0 && frame.cells[a - 1].width == 2 {
            a -= 1;
        }
        let mut text = String::new();
        for y in a / cols..=b / cols {
            let mut line = String::new();
            for x in 0..cols {
                let i = y * cols + x;
                if i < a || i > b {
                    continue;
                }
                let cell = &frame.cells[i];
                if cell.width == 0 {
                    continue;
                }
                if cell.text().is_empty() {
                    line.push(' ');
                } else {
                    line.push_str(cell.text());
                }
            }
            let wrapped = frame.cells[y * cols].flags & 4 != 0;
            text.push_str(if wrapped { &line } else { line.trim_end() });
            if y < b / cols && !wrapped {
                text.push('\n');
            }
        }
        text
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;
    #[test]
    fn soft_wraps_are_joined_and_half_wide_character_is_copied_whole() {
        let mut t = Terminal::new(6, 3).unwrap();
        t.write(b"123456789ABC");
        assert_eq!(
            Selection {
                anchor: (0, 0),
                head: (5, 1)
            }
            .text(&t.frame().unwrap()),
            "123456789ABC"
        );
        let mut t = Terminal::new(6, 3).unwrap();
        t.write("你好".as_bytes());
        assert_eq!(
            Selection {
                anchor: (1, 0),
                head: (3, 0)
            }
            .text(&t.frame().unwrap()),
            "你好"
        );
    }
    #[test]
    fn reversed_selection_preserves_chinese_without_spacer_cells() {
        let mut t = Terminal::new(12, 3).unwrap();
        t.write("A你好 B\r\nsecond".as_bytes());
        let frame = t.frame().unwrap();
        let selection = Selection {
            anchor: (5, 1),
            head: (1, 0),
        };
        assert_eq!(selection.text(&frame), "你好 B\nsecond");
    }
}
