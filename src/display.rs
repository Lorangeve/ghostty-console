use drm::{
    buffer::{Buffer, DrmFourcc},
    control::{self, Device as _, connector, crtc, dumbbuffer::DumbBuffer, framebuffer},
};
use std::{
    cell::RefCell,
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd},
    path::Path,
    rc::Rc,
};
#[link(name = "drm")]
unsafe extern "C" {
    fn drmIsMaster(fd: libc::c_int) -> libc::c_int;
}
pub type SharedSeat = Rc<RefCell<crate::seat::Seat>>;
struct Card {
    device: Option<crate::seat::Device>,
    seat: SharedSeat,
}
impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.device.as_ref().unwrap().as_fd()
    }
}
impl drm::Device for Card {}
impl control::Device for Card {}
impl Drop for Card {
    fn drop(&mut self) {
        if let Some(d) = self.device.take() {
            let _ = self.seat.borrow_mut().close_device(d);
        }
    }
}
pub struct Display {
    card: Card,
    connector: connector::Handle,
    crtc: crtc::Info,
    old_connectors: Vec<connector::Handle>,
    mode: control::Mode,
    buffers: Vec<(DumbBuffer, framebuffer::Handle)>,
    front: usize,
    pending: bool,
    initial: bool,
    pub width: usize,
    pub height: usize,
}
impl Display {
    pub fn open(seat: SharedSeat, path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let device = seat.borrow_mut().open_device(path)?;
        let card = Card {
            device: Some(device),
            seat,
        };
        // seatd may return an fd before the previous logind compositor has
        // released DRM master. Close it and retry through the broker later.
        if unsafe { drmIsMaster(card.as_fd().as_raw_fd()) } != 1 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Waiting for DRM master handoff",
            )
            .into());
        }
        let resources = card.resource_handles()?;
        let connector = resources
            .connectors()
            .iter()
            .filter_map(|h| card.get_connector(*h, true).ok())
            .find(|c| c.state() == connector::State::Connected && !c.modes().is_empty())
            .ok_or("No connected display")?;
        let mode = *connector.modes().first().ok_or("No display mode")?;
        let mut compatible = Vec::new();
        for encoder in connector.encoders() {
            let info = card.get_encoder(*encoder)?;
            compatible.extend(resources.filter_crtcs(info.possible_crtcs()));
        }
        let current = connector
            .current_encoder()
            .and_then(|h| card.get_encoder(h).ok())
            .and_then(|e| e.crtc());
        let handle = current
            .filter(|h| compatible.contains(h))
            .or_else(|| compatible.first().copied())
            .ok_or("No compatible CRTC")?;
        let crtc = card.get_crtc(handle)?;
        let old_connectors = resources
            .connectors()
            .iter()
            .filter_map(|h| {
                let con = card.get_connector(*h, false).ok()?;
                let enc = card.get_encoder(con.current_encoder()?).ok()?;
                if enc.crtc() == Some(handle) {
                    Some(*h)
                } else {
                    None
                }
            })
            .collect();
        let (w, h) = mode.size();
        let mut display = Self {
            card,
            connector: connector.handle(),
            crtc,
            old_connectors,
            mode,
            buffers: Vec::new(),
            front: 0,
            pending: false,
            initial: true,
            width: usize::from(w),
            height: usize::from(h),
        };
        for _ in 0..2 {
            let buffer =
                display
                    .card
                    .create_dumb_buffer((w.into(), h.into()), DrmFourcc::Xrgb8888, 32)?;
            let fb = match display.card.add_framebuffer(&buffer, 24, 32) {
                Ok(fb) => fb,
                Err(e) => {
                    let _ = display.card.destroy_dumb_buffer(buffer);
                    return Err(e.into());
                }
            };
            display.buffers.push((buffer, fb));
        }
        Ok(display)
    }
    pub fn fd(&self) -> i32 {
        self.card.as_fd().as_raw_fd()
    }
    pub fn busy(&self) -> bool {
        self.pending
    }
    pub fn events(&mut self) -> io::Result<()> {
        for event in self.card.receive_events()? {
            if matches!(event, control::Event::PageFlip(_)) {
                self.pending = false;
            }
        }
        Ok(())
    }
    pub fn present(&mut self, pixels: &[u32]) -> io::Result<()> {
        if self.pending {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Page flip pending",
            ));
        }
        if pixels.len() != self.width * self.height {
            return Err(io::Error::other("Invalid frame dimensions"));
        }
        let index = 1 - self.front;
        let (buffer, fb) = &mut self.buffers[index];
        let pitch = buffer.pitch() as usize;
        {
            let mut map = self.card.map_dumb_buffer(buffer)?;
            for y in 0..self.height {
                for x in 0..self.width {
                    let offset = y * pitch + x * 4;
                    map[offset..offset + 4]
                        .copy_from_slice(&pixels[y * self.width + x].to_ne_bytes());
                }
            }
        }
        if self.initial {
            self.card.set_crtc(
                self.crtc.handle(),
                Some(*fb),
                (0, 0),
                &[self.connector],
                Some(self.mode),
            )?;
            self.initial = false;
        } else {
            self.card
                .page_flip(self.crtc.handle(), *fb, control::PageFlipFlags::EVENT, None)?;
            self.pending = true;
        }
        self.front = index;
        Ok(())
    }
}
impl Drop for Display {
    fn drop(&mut self) {
        if !self.initial {
            let _ = self.card.set_crtc(
                self.crtc.handle(),
                self.crtc.framebuffer(),
                self.crtc.position(),
                &self.old_connectors,
                self.crtc.mode(),
            );
        }
        for (buffer, fb) in self.buffers.drain(..) {
            let _ = self.card.destroy_framebuffer(fb);
            let _ = self.card.destroy_dumb_buffer(buffer);
        }
    }
}

/// Device revocation is a seat transition, never a reason to log the user out.
pub fn device_transition(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EACCES | libc::EPERM | libc::ENODEV | libc::EIO | libc::EAGAIN)
    )
}
#[cfg(test)]
mod transition_tests {
    #[test]
    fn permission_revocation_is_recoverable() {
        for code in [
            libc::EACCES,
            libc::EPERM,
            libc::ENODEV,
            libc::EIO,
            libc::EAGAIN,
        ] {
            assert!(super::device_transition(
                &std::io::Error::from_raw_os_error(code)
            ));
        }
        assert!(!super::device_transition(
            &std::io::Error::from_raw_os_error(libc::EINVAL)
        ));
    }
}
