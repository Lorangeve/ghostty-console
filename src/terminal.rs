use std::{
    ffi::{c_char, c_int, c_void},
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
};
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub text: [u8; 64],
    pub fg: u32,
    pub bg: u32,
    pub width: u8,
    pub flags: u8,
}
impl Default for Cell {
    fn default() -> Self {
        Self {
            text: [0; 64],
            fg: 0xffffff,
            bg: 0,
            width: 1,
            flags: 0,
        }
    }
}
impl Cell {
    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.text[..self.text.iter().position(|b| *b == 0).unwrap_or(64)])
            .unwrap_or("�")
    }
}
unsafe extern "C" {
    fn gt_new(
        cols: u16,
        rows: u16,
        reply: unsafe extern "C" fn(*mut c_void, *const u8, usize),
        data: *mut c_void,
    ) -> *mut c_void;
    fn gt_cell_size(v: *mut c_void, width: u32, height: u32) -> i32;
    fn gt_resize(v: *mut c_void, cols: u16, rows: u16) -> i32;
    fn gt_images_begin(v: *mut c_void) -> i32;
    fn gt_image_next(v: *mut c_void, image: *mut RawImage) -> i32;
    fn gt_clipboard(v: *mut c_void, out: *mut *const u8, len: *mut usize) -> i32;
    fn gt_mouse_tracking(v: *mut c_void) -> i32;
    fn gt_bracketed_paste(v: *mut c_void) -> i32;
    fn gt_mouse(
        v: *mut c_void,
        action: i32,
        button: i32,
        x: f32,
        y: f32,
        mods: u32,
        out: *mut u8,
        cap: usize,
    ) -> i32;
    fn gt_colors(v: *mut c_void, fg: u32, bg: u32, cursor: u32);
    fn gt_free(v: *mut c_void);
    fn gt_write(v: *mut c_void, data: *const u8, len: usize);
    fn gt_frame(
        v: *mut c_void,
        cells: *mut Cell,
        capacity: usize,
        x: *mut c_int,
        y: *mut c_int,
        caret_x: *mut c_int,
        caret_y: *mut c_int,
    ) -> c_int;
    fn gt_key(
        v: *mut c_void,
        evdev: u32,
        action: i32,
        mods: u32,
        text: *const c_char,
        len: usize,
        unshifted: u32,
        out: *mut u8,
        cap: usize,
    ) -> i32;
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct RawImage {
    col: i32,
    row: i32,
    z: i32,
    layer: i32,
    offset_x: u32,
    offset_y: u32,
    dest_width: u32,
    dest_height: u32,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
    format: u32,
    id: u32,
    generation: u64,
    storage_id: u64,
    pixels: *const u8,
    len: usize,
}
#[derive(Clone)]
pub struct Image {
    pub col: i32,
    pub row: i32,
    pub z: i32,
    pub layer: i32,
    pub offset_x: u32,
    pub offset_y: u32,
    pub dest_width: u32,
    pub dest_height: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub pixels: std::sync::Arc<[u8]>,
}
pub struct Terminal {
    raw: NonNull<c_void>,
    // The Vec object itself must have a stable address for the native callback.
    #[allow(clippy::box_collection)]
    replies: Box<Vec<u8>>,
    pub cols: u16,
    pub rows: u16,
    image_cache: std::collections::HashMap<(u64, u32, u64), std::sync::Arc<[u8]>>,
    _thread: PhantomData<Rc<()>>,
}
pub struct Frame {
    pub cells: Vec<Cell>,
    pub cols: u16,
    pub rows: u16,
    pub cursor: Option<(usize, usize)>,
    pub caret: Option<(usize, usize)>,
    pub images: Vec<Image>,
    pub mouse_tracking: bool,
}
unsafe extern "C" fn reply(data: *mut c_void, bytes: *const u8, len: usize) {
    // SAFETY: native callback is synchronous; Box allocation is stable for the terminal lifetime.
    let output = unsafe { &mut *data.cast::<Vec<u8>>() };
    if len <= 1024 * 1024 && output.len() + len <= 1024 * 1024 {
        output.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, len) });
    }
}
impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Result<Self, String> {
        if cols == 0 || rows == 0 {
            return Err("Empty terminal dimensions".into());
        }
        let settings = crate::settings::get()?;
        let mut replies = Box::new(Vec::new());
        let raw = unsafe { gt_new(cols, rows, reply, (&mut *replies as *mut Vec<u8>).cast()) };
        let raw = NonNull::new(raw).ok_or("Cannot initialize libghostty-vt")?;
        unsafe {
            gt_colors(
                raw.as_ptr(),
                settings.foreground.unwrap_or(u32::MAX),
                settings.background.unwrap_or(u32::MAX),
                settings.cursor_color,
            );
        }
        Ok(Self {
            raw,
            replies,
            cols,
            rows,
            image_cache: std::collections::HashMap::new(),
            _thread: PhantomData,
        })
    }
    pub fn set_cell_size(&mut self, width: usize, height: usize) -> Result<(), String> {
        if unsafe { gt_cell_size(self.raw.as_ptr(), width as u32, height as u32) } != 0 {
            return Err("Cannot set terminal pixel geometry".into());
        }
        Ok(())
    }
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        if cols == 0 || rows == 0 {
            return Err("Empty terminal geometry".into());
        }
        if unsafe { gt_resize(self.raw.as_ptr(), cols, rows) } != 0 {
            return Err("Cannot resize terminal".into());
        }
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }
    pub fn mouse_tracking(&self) -> bool {
        unsafe { gt_mouse_tracking(self.raw.as_ptr()) != 0 }
    }
    pub fn paste(&self, text: &str) -> Vec<u8> {
        let text = text.replace('\u{1b}', "");
        if unsafe { gt_bracketed_paste(self.raw.as_ptr()) != 0 } {
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.into_bytes()
        }
    }
    pub fn mouse(
        &mut self,
        action: i32,
        button: i32,
        x: f32,
        y: f32,
        mods: u32,
    ) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0; 128];
        let len = unsafe {
            gt_mouse(
                self.raw.as_ptr(),
                action,
                button,
                x,
                y,
                mods,
                bytes.as_mut_ptr(),
                bytes.len(),
            )
        };
        if len < 0 {
            return Err("Cannot encode mouse event".into());
        }
        bytes.truncate(len as usize);
        Ok(bytes)
    }
    fn images(&mut self) -> Result<Vec<Image>, String> {
        if unsafe { gt_images_begin(self.raw.as_ptr()) } != 0 {
            return Ok(Vec::new());
        }
        let mut images = Vec::new();
        let mut alive = std::collections::HashSet::new();
        let mut bytes = 0usize;
        loop {
            let mut raw = RawImage::default();
            if unsafe { gt_image_next(self.raw.as_ptr(), &mut raw) } != 1 {
                break;
            }
            let bpp = match raw.format {
                0 => 3,
                1 => 4,
                3 => 2,
                4 => 1,
                _ => continue,
            };
            let expected = (raw.width as usize)
                .checked_mul(raw.height as usize)
                .and_then(|n| n.checked_mul(bpp));
            if expected != Some(raw.len) || raw.pixels.is_null() || raw.len > 64 * 1024 * 1024 {
                continue;
            }
            if images.len() >= 512 {
                return Err("Too many visible image placements".into());
            }
            let key = (raw.storage_id, raw.id, raw.generation);
            alive.insert(key);
            let pixels = if let Some(cached) = self.image_cache.get(&key) {
                cached.clone()
            } else {
                bytes = bytes.saturating_add(raw.len);
                if bytes > 64 * 1024 * 1024 {
                    return Err("Image snapshot exceeds memory budget".into());
                }
                // SAFETY: native pixels belong to the terminal and no mutation occurs while copying.
                let pixels: std::sync::Arc<[u8]> =
                    unsafe { std::slice::from_raw_parts(raw.pixels, raw.len) }.into();
                self.image_cache.insert(key, pixels.clone());
                pixels
            };
            images.push(Image {
                col: raw.col,
                row: raw.row,
                z: raw.z,
                layer: raw.layer,
                offset_x: raw.offset_x,
                offset_y: raw.offset_y,
                dest_width: raw.dest_width,
                dest_height: raw.dest_height,
                source_x: raw.source_x,
                source_y: raw.source_y,
                source_width: raw.source_width,
                source_height: raw.source_height,
                width: raw.width,
                height: raw.height,
                format: raw.format,
                pixels,
            });
        }
        self.image_cache.retain(|key, _| alive.contains(key));
        images.sort_by_key(|image| (image.layer, image.z));
        Ok(images)
    }
    pub fn write(&mut self, bytes: &[u8]) {
        unsafe {
            gt_write(self.raw.as_ptr(), bytes.as_ptr(), bytes.len());
        }
    }
    pub fn take_clipboard(&mut self) -> Option<String> {
        let mut data = std::ptr::null();
        let mut len = 0;
        if unsafe { gt_clipboard(self.raw.as_ptr(), &mut data, &mut len) } != 1 {
            return None;
        }
        if data.is_null() || len > 1024 * 1024 {
            return None;
        }
        // SAFETY: native allocation remains live until the next terminal mutation or drop.
        String::from_utf8(unsafe { std::slice::from_raw_parts(data, len) }.to_vec()).ok()
    }
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }
    pub fn frame(&mut self) -> Result<Frame, String> {
        let mut cells = vec![Cell::default(); usize::from(self.cols) * usize::from(self.rows)];
        let (mut x, mut y) = (-1, -1);
        let (mut caret_x, mut caret_y) = (-1, -1);
        let count = unsafe {
            gt_frame(
                self.raw.as_ptr(),
                cells.as_mut_ptr(),
                cells.len(),
                &mut x,
                &mut y,
                &mut caret_x,
                &mut caret_y,
            )
        };
        if count != cells.len() as i32 {
            return Err(format!("Invalid terminal snapshot size: {count}"));
        }
        let images = self.images()?;
        Ok(Frame {
            mouse_tracking: self.mouse_tracking(),
            caret: if caret_x >= 0 && caret_y >= 0 {
                Some((caret_x as usize, caret_y as usize))
            } else {
                None
            },
            cells,
            images,
            cols: self.cols,
            rows: self.rows,
            cursor: if x >= 0 && y >= 0 {
                Some((x as usize, y as usize))
            } else {
                None
            },
        })
    }
    pub fn key(
        &mut self,
        evdev: u32,
        action: i32,
        mods: u32,
        text: &str,
        unshifted: u32,
    ) -> Result<Vec<u8>, String> {
        let mut output = vec![0; 4096];
        let len = unsafe {
            gt_key(
                self.raw.as_ptr(),
                evdev,
                action,
                mods,
                text.as_ptr().cast(),
                text.len(),
                unshifted,
                output.as_mut_ptr(),
                output.len(),
            )
        };
        if len < 0 {
            return Err("Cannot encode key event".into());
        }
        output.truncate(len as usize);
        Ok(output)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            gt_free(self.raw.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_protocol_size_queries_and_updates() {
        let mut t = Terminal::new(20, 10).unwrap();
        t.set_cell_size(10, 20).unwrap();
        t.write(b"\x1b[14t\x1b[16t\x1b[18t\x1b[c");
        let replies = String::from_utf8(t.take_replies()).unwrap();
        assert!(replies.contains("\x1b[4;200;200t"), "{replies:?}");
        assert!(replies.contains("\x1b[6;20;10t"), "{replies:?}");
        t.write(b"\x1b_Gi=4207,a=q,t=d,f=24,s=1,v=1;AAAA\x1b\\");
        assert!(
            String::from_utf8(t.take_replies())
                .unwrap()
                .contains("i=4207;OK")
        );
        t.write(b"\x1b_Ga=T,f=24,s=2,v=1,c=2,r=1,i=10;/wAAAP8A\x1b\\");
        let frame = t.frame().unwrap();
        assert_eq!(frame.images.len(), 1);
        assert_eq!(&*frame.images[0].pixels, &[255, 0, 0, 0, 255, 0]);
        assert_eq!(
            (frame.images[0].dest_width, frame.images[0].dest_height),
            (20, 20)
        );
        t.write(b"\x1b_Ga=T,f=24,s=2,v=1,c=2,r=1,i=10;AP8A/wAA\x1b\\");
        assert_eq!(
            &*t.frame().unwrap().images[0].pixels,
            &[0, 255, 0, 255, 0, 0]
        );
        t.write(b"\x1b_Ga=d,d=A\x1b\\");
        assert!(t.frame().unwrap().images.is_empty());
        t.resize(20, 13).unwrap();
        t.write(b"\x1b[14t");
        assert!(
            String::from_utf8(t.take_replies())
                .unwrap()
                .contains("\x1b[4;260;200t")
        );
    }
    #[test]
    fn mouse_tracking_and_bracketed_paste_follow_app_modes() {
        let mut t = Terminal::new(20, 10).unwrap();
        t.set_cell_size(10, 20).unwrap();
        assert!(!t.mouse_tracking());
        t.write(b"\x1b[?1000h\x1b[?1006h\x1b[?2004h");
        assert!(t.mouse_tracking());
        assert_eq!(t.mouse(0, 1, 10., 20., 0).unwrap(), b"\x1b[<0;2;2M");
        assert_eq!(t.mouse(0, 4, 10., 20., 0).unwrap(), b"\x1b[<64;2;2M");
        assert_eq!(t.paste("你好"), "\x1b[200~你好\x1b[201~".as_bytes());
        t.write(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(t.take_clipboard(), Some("hello".into()));
        assert_eq!(t.take_clipboard(), None);
    }
    #[test]
    fn actual_caret_is_available_even_when_the_application_hides_it() {
        let mut terminal = Terminal::new(80, 24).unwrap();
        terminal.write(b"\x1b[6;18H\x1b[?25l");
        let frame = terminal.frame().unwrap();
        assert_eq!(frame.caret, Some((17, 5)));
        assert_eq!(frame.cursor, None);
    }
    #[test]
    fn unicode_colors_queries_and_keyboard() {
        let mut t = Terminal::new(20, 4).unwrap();
        t.write("\x1b[31mA\x1b[0m你好".as_bytes());
        let f = t.frame().unwrap();
        assert_eq!(f.cells[0].text(), "A");
        assert_ne!(f.cells[0].fg, f.cells[1].fg);
        assert_eq!(f.cells[1].text(), "你");
        assert_eq!(f.cells[1].width, 2);
        assert_eq!(f.cells[2].width, 0);
        t.write(b"\x1b[6n");
        assert!(!t.take_replies().is_empty());
        assert_eq!(t.key(28, 1, 0, "\r", 13).unwrap(), b"\r");
        assert_eq!(t.key(46, 1, 2, "c", 99).unwrap(), b"\x03");
        t.write(b"\x1b[?1049hALT\x1b[?1049l");
        assert_eq!(t.frame().unwrap().cells[0].text(), "A");
    }
}
