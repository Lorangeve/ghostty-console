//! Existing logind backend for manual con, private inherited broker for system login.
use crate::broker::{receive_fd, send_fd};
use std::{
    fs::File,
    io,
    os::fd::{AsFd, BorrowedFd},
    os::unix::net::UnixDatagram,
    path::Path,
};
pub enum Device {
    Logind(libseat::Device),
    Native { file: File, id: u32 },
}
impl AsFd for Device {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            Self::Logind(d) => d.as_fd(),
            Self::Native { file, .. } => file.as_fd(),
        }
    }
}
pub enum Seat {
    Logind(libseat::Seat),
    Native {
        rpc: UnixDatagram,
        events: UnixDatagram,
        callback: Box<dyn FnMut(libseat::SeatEvent)>,
    },
}
fn errno(e: impl Into<i32>) -> io::Error {
    io::Error::from_raw_os_error(e.into())
}
impl Seat {
    pub fn logind(mut callback: impl FnMut(libseat::SeatEvent) + 'static) -> io::Result<Self> {
        libseat::Seat::open(move |_, event| callback(event))
            .map(Self::Logind)
            .map_err(|e| errno(e.0))
    }
    pub fn native(
        rpc: UnixDatagram,
        events: UnixDatagram,
        callback: impl FnMut(libseat::SeatEvent) + 'static,
    ) -> io::Result<Self> {
        events.set_nonblocking(true)?;
        rpc.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        Ok(Self::Native {
            rpc,
            events,
            callback: Box::new(callback),
        })
    }
    fn request(&self, bytes: &[u8]) -> io::Result<(u32, Option<File>)> {
        let Self::Native { rpc, .. } = self else {
            return Err(io::Error::other("Not a native seat"));
        };
        send_fd(rpc, bytes, None)?;
        let (reply, fd) = receive_fd(rpc)?;
        if reply.len() != 8 {
            return Err(io::Error::other("Invalid broker reply"));
        }
        let code = i32::from_le_bytes(reply[..4].try_into().unwrap());
        if code != 0 {
            return Err(errno(code));
        }
        Ok((u32::from_le_bytes(reply[4..8].try_into().unwrap()), fd))
    }
    pub fn open_device(&mut self, path: &Path) -> io::Result<Device> {
        match self {
            Self::Logind(s) => s
                .open_device(&path)
                .map(Device::Logind)
                .map_err(|e| errno(e.0)),
            Self::Native { .. } => {
                use std::os::unix::ffi::OsStrExt;
                let mut bytes = vec![b'O'];
                bytes.extend_from_slice(path.as_os_str().as_bytes());
                let (id, file) = self.request(&bytes)?;
                Ok(Device::Native {
                    id,
                    file: file.ok_or_else(|| io::Error::other("Broker omitted fd"))?,
                })
            }
        }
    }
    pub fn close_device(&mut self, device: Device) -> io::Result<()> {
        match (self, device) {
            (Self::Logind(s), Device::Logind(d)) => s.close_device(d).map_err(|e| errno(e.0)),
            (s @ Self::Native { .. }, Device::Native { file, id }) => {
                drop(file);
                let mut req = vec![b'C'];
                req.extend_from_slice(&id.to_le_bytes());
                s.request(&req).map(|_| ())
            }
            _ => Err(io::Error::other("Mismatched seat device")),
        }
    }
    pub fn dispatch(&mut self, timeout: i32) -> io::Result<()> {
        match self {
            Self::Logind(s) => s.dispatch(timeout).map(|_| ()).map_err(|e| errno(e.0)),
            Self::Native {
                events, callback, ..
            } => {
                let mut b = [0; 1];
                loop {
                    match events.recv(&mut b) {
                        Ok(1) => match b[0] {
                            b'E' => callback(libseat::SeatEvent::Enable),
                            b'D' => callback(libseat::SeatEvent::Disable),
                            _ => return Err(io::Error::other("Invalid seat event")),
                        },
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => return Err(e),
                        _ => return Err(io::Error::other("Seat broker closed")),
                    }
                }
                Ok(())
            }
        }
    }
    pub fn disable(&mut self) -> io::Result<()> {
        match self {
            Self::Logind(s) => s.disable().map_err(|e| errno(e.0)),
            s @ Self::Native { .. } => s.request(b"D").map(|_| ()),
        }
    }
    pub fn switch_session(&mut self, vt: i32) -> io::Result<()> {
        match self {
            Self::Logind(s) => s.switch_session(vt).map_err(|e| errno(e.0)),
            s @ Self::Native { .. } => {
                let mut b = vec![b'S'];
                b.extend_from_slice(&vt.to_le_bytes());
                s.request(&b).map(|_| ())
            }
        }
    }
    pub fn get_fd(&mut self) -> io::Result<BorrowedFd<'_>> {
        match self {
            Self::Logind(s) => s.get_fd().map_err(|e| errno(e.0)),
            Self::Native { events, .. } => {
                Ok(unsafe { BorrowedFd::borrow_raw(std::os::fd::AsRawFd::as_raw_fd(events)) })
            }
        }
    }
}
