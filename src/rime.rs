//! Single-owner librime adapter. All returned strings are copied before the next key.
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    marker::PhantomData,
    path::Path,
    ptr::NonNull,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

unsafe extern "C" {
    fn cr_open(
        shared: *const c_char,
        user: *const c_char,
        prebuilt: *const c_char,
        schema: *const c_char,
    ) -> *mut c_void;
    fn cr_clear(r: *mut c_void);
    fn cr_close(r: *mut c_void);
    fn cr_key(r: *mut c_void, key: c_int, modifiers: c_int) -> c_int;
    fn cr_commit(r: *mut c_void) -> *const c_char;
    fn cr_preedit(r: *mut c_void) -> *const c_char;
    fn cr_meta(r: *mut c_void, field: c_int) -> c_int;
    fn cr_candidate(r: *mut c_void, index: c_int, field: c_int) -> *const c_char;
}
static ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
    pub label: String,
}
#[derive(Debug)]
pub struct Update {
    pub handled: bool,
    pub commit: String,
    pub preedit: String,
    /// Rime offsets are UTF-8 byte offsets, not character indexes.
    pub cursor: usize,
    pub selection: std::ops::Range<usize>,
    pub page: usize,
    pub highlighted: usize,
    pub last_page: bool,
    pub candidates: Vec<Candidate>,
}

pub struct Rime {
    raw: NonNull<c_void>,
    // Keep traits' strings alive for the complete native engine lifetime.
    _strings: [CString; 4],
    _data_lock: std::fs::File,
    _single_thread: PhantomData<Rc<()>>,
}
impl Rime {
    pub fn clear(&mut self) {
        unsafe {
            cr_clear(self.raw.as_ptr());
        }
    }
    pub fn open(shared: &Path, user: &Path, prebuilt: &Path, schema: &str) -> Result<Self, String> {
        use std::os::unix::ffi::OsStrExt;
        let data_lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(user.join(".ghostty-console.lock"))
            .map_err(|e| e.to_string())?;
        data_lock
            .try_lock()
            .map_err(|e| format!("Rime data is already in use or cannot be locked: {e}"))?;
        let strings = [
            CString::new(shared.as_os_str().as_bytes()),
            CString::new(user.as_os_str().as_bytes()),
            CString::new(prebuilt.as_os_str().as_bytes()),
            CString::new(schema),
        ];
        let strings: Vec<CString> = strings
            .into_iter()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let strings: [CString; 4] = strings.try_into().unwrap();
        if ACTIVE.swap(true, Ordering::AcqRel) {
            return Err("Only one Rime engine may exist in this process".into());
        }
        // SAFETY: all arguments are live NUL-terminated strings; global ownership is exclusive.
        let raw = unsafe {
            cr_open(
                strings[0].as_ptr(),
                strings[1].as_ptr(),
                strings[2].as_ptr(),
                strings[3].as_ptr(),
            )
        };
        match NonNull::new(raw) {
            Some(raw) => Ok(Self {
                raw,
                _strings: strings,
                _data_lock: data_lock,
                _single_thread: PhantomData,
            }),
            None => {
                ACTIVE.store(false, Ordering::Release);
                Err(format!(
                    "Cannot load Rime schema {schema}; check compiled data"
                ))
            }
        }
    }
    /// key is an XKB keysym; modifiers use Rime/X11 masks (Shift=1, Ctrl=4, Alt=8, release=1<<30).
    pub fn process_key(&mut self, key: i32, modifiers: i32) -> Update {
        let r = self.raw.as_ptr();
        // SAFETY: the native object is exclusively owned and all pointers are copied before mutation.
        unsafe {
            let handled = cr_key(r, key, modifiers) != 0;
            let candidates = (0..cr_meta(r, 3))
                .map(|i| Candidate {
                    text: copy(cr_candidate(r, i, 0)),
                    comment: copy(cr_candidate(r, i, 1)),
                    label: copy(cr_candidate(r, i, 2)),
                })
                .collect();
            Update {
                handled,
                commit: copy(cr_commit(r)),
                preedit: copy(cr_preedit(r)),
                cursor: cr_meta(r, 0).max(0) as usize,
                selection: cr_meta(r, 5).max(0) as usize..cr_meta(r, 6).max(0) as usize,
                page: cr_meta(r, 1).max(0) as usize,
                highlighted: cr_meta(r, 2).max(0) as usize,
                last_page: cr_meta(r, 4) != 0,
                candidates,
            }
        }
    }
}
unsafe fn copy(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}
impl Drop for Rime {
    fn drop(&mut self) {
        // SAFETY: unique ownership, native state is destroyed before traits strings.
        unsafe {
            cr_close(self.raw.as_ptr());
        }
        ACTIVE.store(false, Ordering::Release);
    }
}
