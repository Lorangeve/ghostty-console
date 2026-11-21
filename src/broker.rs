//! Fixed tty6 root broker. No terminal/font/IME parsing and no public socket.
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io, mem,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            net::UnixDatagram,
        },
    },
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[link(name = "drm")]
unsafe extern "C" {
    fn drmSetMaster(fd: i32) -> i32;
    fn drmDropMaster(fd: i32) -> i32;
}
/// SOCK_DGRAM preserves message boundaries; fd reception always uses CLOEXEC.
pub fn send_fd(socket: &UnixDatagram, bytes: &[u8], fd: Option<RawFd>) -> io::Result<()> {
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr().cast_mut().cast(),
        iov_len: bytes.len(),
    };
    let mut control = [0usize; 8];
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if let Some(fd) = fd {
        unsafe {
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = libc::CMSG_SPACE(mem::size_of::<RawFd>() as _) as usize;
            let header = libc::CMSG_FIRSTHDR(&msg);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(mem::size_of::<RawFd>() as _) as usize;
            std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<RawFd>(), fd);
        }
    }
    let n = unsafe { libc::sendmsg(socket.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) };
    if n == bytes.len() as isize {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
pub fn receive_fd(socket: &UnixDatagram) -> io::Result<(Vec<u8>, Option<File>)> {
    let mut bytes = [0u8; 1024];
    let mut control = [0usize; 8];
    let mut iov = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    };
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = mem::size_of_val(&control);
    let n = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut files = Vec::new();
    unsafe {
        let mut h = libc::CMSG_FIRSTHDR(&msg);
        while !h.is_null() {
            if (*h).cmsg_level == libc::SOL_SOCKET && (*h).cmsg_type == libc::SCM_RIGHTS {
                let base = libc::CMSG_LEN(0) as usize;
                if (*h).cmsg_len >= base {
                    for i in 0..((*h).cmsg_len - base) / mem::size_of::<RawFd>() {
                        let fd =
                            std::ptr::read_unaligned(libc::CMSG_DATA(h).cast::<RawFd>().add(i));
                        files.push(File::from_raw_fd(fd));
                    }
                }
            }
            h = libc::CMSG_NXTHDR(&msg, h);
        }
    }
    if msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 || files.len() > 1 {
        return Err(io::Error::other("Truncated/invalid broker packet"));
    }
    Ok((bytes[..n as usize].to_vec(), files.pop()))
}
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct VtMode {
    mode: u8,
    waitv: u8,
    relsig: i16,
    acqsig: i16,
    frsig: i16,
}
struct OpenDevice {
    file: File,
    drm: bool,
}
impl OpenDevice {
    fn revoke(&self) {
        unsafe {
            if self.drm {
                drmDropMaster(self.file.as_raw_fd());
            } else {
                libc::ioctl(self.file.as_raw_fd(), 0x40044591u64, 0);
            }
        }
    }
}
pub struct NativeBroker {
    tty: File,
    old: VtMode,
    release: Arc<AtomicBool>,
    acquire: Arc<AtomicBool>,
    rpc: Option<UnixDatagram>,
    events: Option<UnixDatagram>,
    devices: HashMap<u32, OpenDevice>,
    next: u32,
    pub active: bool,
    pending_release: Option<Instant>,
    signals: Vec<signal_hook::SigId>,
}
pub fn device_kind(path: &Path) -> Option<bool> {
    let name = path.file_name()?.to_str()?;
    let (prefix, drm) = if path.parent()? == Path::new("/dev/dri") {
        ("card", true)
    } else if path.parent()? == Path::new("/dev/input") {
        ("event", false)
    } else {
        return None;
    };
    let suffix = name.strip_prefix(prefix)?;
    (!suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit())).then_some(drm)
}
fn active_vt() -> io::Result<bool> {
    Ok(std::fs::read_to_string("/sys/class/tty/tty0/active")?.trim() == "tty6")
}
impl NativeBroker {
    pub fn new() -> io::Result<Self> {
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC)
            .open("/dev/tty6")?;
        let mut old = VtMode::default();
        if unsafe { libc::ioctl(tty.as_raw_fd(), 0x5601, &mut old) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let release = Arc::new(AtomicBool::new(false));
        let acquire = Arc::new(AtomicBool::new(false));
        let signals = vec![
            signal_hook::flag::register(libc::SIGUSR1, release.clone())?,
            signal_hook::flag::register(libc::SIGUSR2, acquire.clone())?,
        ];
        let mode = VtMode {
            mode: 1,
            relsig: libc::SIGUSR1 as _,
            acqsig: libc::SIGUSR2 as _,
            ..VtMode::default()
        };
        if unsafe { libc::ioctl(tty.as_raw_fd(), 0x5602, &mode) } < 0 {
            for id in signals {
                signal_hook::low_level::unregister(id);
            }
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            tty,
            old,
            release,
            acquire,
            rpc: None,
            events: None,
            devices: HashMap::new(),
            next: 1,
            active: active_vt()?,
            pending_release: None,
            signals,
        })
    }
    fn graphics(&self) -> io::Result<()> {
        let fd = self.tty.as_raw_fd();
        let mut raw = unsafe { mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut raw) } < 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } < 0
            || unsafe { libc::ioctl(fd, 0x4b45, 4) } < 0
            || unsafe { libc::ioctl(fd, 0x4b3a, 1) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn attach(&mut self, rpc: UnixDatagram, events: UnixDatagram) -> io::Result<()> {
        self.detach();
        rpc.set_nonblocking(true)?;
        events.set_nonblocking(true)?;
        self.rpc = Some(rpc);
        self.events = Some(events);
        if self.active && active_vt()? {
            self.graphics()?;
            self.event(b'E');
        }
        Ok(())
    }
    fn event(&self, b: u8) {
        if let Some(s) = &self.events {
            let _ = s.send(&[b]);
        }
    }
    pub fn detach(&mut self) {
        for d in self.devices.values() {
            d.revoke();
        }
        self.devices.clear();
        self.rpc = None;
        self.events = None;
    }
    fn ack_release(&mut self) {
        unsafe {
            libc::ioctl(self.tty.as_raw_fd(), 0x5605, 1);
        }
        self.pending_release = None;
    }
    pub fn tick(&mut self) -> io::Result<()> {
        if self.release.swap(false, Ordering::SeqCst) {
            self.active = false;
            for d in self.devices.values() {
                d.revoke();
            }
            self.pending_release = Some(Instant::now());
            self.event(b'D');
            if self.events.is_none() {
                self.ack_release();
            }
        }
        if self.acquire.swap(false, Ordering::SeqCst) {
            unsafe {
                libc::ioctl(self.tty.as_raw_fd(), 0x5605, 2);
            }
            self.active = active_vt()?;
            if self.active {
                self.graphics()?;
                self.event(b'E');
            }
        }
        // A wedged renderer cannot hold the desktop hostage. The supervisor
        // terminates that renderer when this deadline expires.
        if self
            .pending_release
            .is_some_and(|t| t.elapsed() > Duration::from_secs(1))
        {
            self.ack_release();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Renderer did not release tty6",
            ));
        }
        for _ in 0..32 {
            let Some(rpc) = &self.rpc else {
                break;
            };
            let (request, received) = match receive_fd(rpc) {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            };
            if received.is_some() {
                self.reply(libc::EINVAL, 0, None)?;
                continue;
            }
            let result = self.handle(&request);
            match result {
                Ok((id, fd)) => self.reply(0, id, fd)?,
                Err(e) => self.reply(e.raw_os_error().unwrap_or(libc::EIO), 0, None)?,
            }
        }
        Ok(())
    }
    fn reply(&self, code: i32, id: u32, fd: Option<RawFd>) -> io::Result<()> {
        let mut b = code.to_le_bytes().to_vec();
        b.extend_from_slice(&id.to_le_bytes());
        send_fd(self.rpc.as_ref().unwrap(), &b, fd)
    }
    fn handle(&mut self, b: &[u8]) -> io::Result<(u32, Option<RawFd>)> {
        match b {
            [b'D'] => {
                if self.pending_release.is_some() {
                    self.ack_release();
                }
                Ok((0, None))
            }
            [b'C', a, b, c, d] => {
                let id = u32::from_le_bytes([*a, *b, *c, *d]);
                if let Some(d) = self.devices.remove(&id) {
                    d.revoke();
                }
                Ok((0, None))
            }
            [b'S', a, b, c, d] => {
                let vt = i32::from_le_bytes([*a, *b, *c, *d]);
                if !(1..=63).contains(&vt) {
                    return Err(io::Error::from_raw_os_error(libc::EINVAL));
                }
                if unsafe { libc::ioctl(self.tty.as_raw_fd(), 0x5606, vt) } < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok((0, None))
            }
            [b'O', path @ ..] => {
                if !self.active || !active_vt()? {
                    return Err(io::Error::from_raw_os_error(libc::EAGAIN));
                }
                let path = std::str::from_utf8(path)
                    .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
                let drm = device_kind(Path::new(path))
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::EACCES))?;
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(
                        libc::O_CLOEXEC | libc::O_NOCTTY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                    )
                    .open(path)?;
                let meta = file.metadata()?;
                let major = libc::major(meta.rdev());
                if !std::os::unix::fs::FileTypeExt::is_char_device(&meta.file_type())
                    || major != if drm { 226 } else { 13 }
                {
                    return Err(io::Error::from_raw_os_error(libc::EACCES));
                }
                if drm && unsafe { drmSetMaster(file.as_raw_fd()) } < 0 {
                    let error = io::Error::last_os_error();
                    return Err(if error.raw_os_error() == Some(libc::EBUSY) {
                        io::Error::from_raw_os_error(libc::EAGAIN)
                    } else {
                        error
                    });
                }
                if self.devices.len() >= 128 {
                    return Err(io::Error::from_raw_os_error(libc::EMFILE));
                }
                let id = self.next;
                self.next = self
                    .next
                    .checked_add(1)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
                let fd = file.as_raw_fd();
                self.devices.insert(id, OpenDevice { file, drm });
                Ok((id, Some(fd)))
            }
            _ => Err(io::Error::from_raw_os_error(libc::EINVAL)),
        }
    }
}
impl Drop for NativeBroker {
    fn drop(&mut self) {
        self.detach();
        if self.pending_release.is_some() {
            self.ack_release();
        }
        unsafe {
            libc::ioctl(self.tty.as_raw_fd(), 0x5602, &self.old);
        }
        for id in self.signals.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_fixed_device_paths_are_allowed() {
        for path in [
            "/etc/shadow",
            "/dev/dri/../tty1",
            "/dev/dri/card",
            "/dev/input/event1/../x",
            "/dev/dri/renderD128",
        ] {
            assert_eq!(device_kind(Path::new(path)), None);
        }
        assert_eq!(device_kind(Path::new("/dev/dri/card1")), Some(true));
        assert_eq!(device_kind(Path::new("/dev/input/event12")), Some(false));
    }
    #[test]
    fn fd_transfer_is_cloexec_and_messages_are_bounded() {
        let (a, b) = UnixDatagram::pair().unwrap();
        let file = File::open("/dev/null").unwrap();
        send_fd(&a, b"test", Some(file.as_raw_fd())).unwrap();
        let (bytes, fd) = receive_fd(&b).unwrap();
        assert_eq!(bytes, b"test");
        assert_ne!(
            unsafe { libc::fcntl(fd.unwrap().as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        a.send(&vec![0u8; 2048]).unwrap();
        assert!(receive_fd(&b).is_err());
    }
}
