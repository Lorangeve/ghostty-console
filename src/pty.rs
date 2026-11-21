use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
    time::Instant,
};

pub struct Pty {
    pub master: File,
    child: Option<Child>,
    pending: VecDeque<u8>,
    reaped: bool,
}
impl Pty {
    pub fn spawn(command: &str, args: &[&str], cols: u16, rows: u16) -> io::Result<Self> {
        Self::spawn_env(command, args, cols, rows, &[])
    }
    pub fn spawn_env(
        command: &str,
        args: &[&str],
        cols: u16,
        rows: u16,
        envs: &[(&str, &str)],
    ) -> io::Result<Self> {
        Self::spawn_inner(command, args, cols, rows, envs, false)
    }
    pub fn spawn_login(vt: u32, cols: u16, rows: u16) -> io::Result<Self> {
        let vt = vt.to_string();
        Self::spawn_inner(
            "/usr/bin/login",
            &["-p", "-s", crate::login_gate::SHELL],
            cols,
            rows,
            &[
                ("XDG_SEAT", "seat0"),
                ("XDG_VTNR", &vt),
                ("XDG_SESSION_TYPE", "tty"),
                ("LANG", "en_US.UTF-8"),
                ("PATH", "/usr/local/bin:/usr/bin"),
                ("GHOSTTY_CONSOLE_LOGIN", "1"),
            ],
            true,
        )
    }
    fn spawn_inner(
        command: &str,
        args: &[&str],
        cols: u16,
        rows: u16,
        envs: &[(&str, &str)],
        login: bool,
    ) -> io::Result<Self> {
        let mut master = -1;
        let mut slave = -1;
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        if unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut cmd = Command::new(command);
        if login {
            cmd.env_clear();
        }
        cmd.args(args)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("NIRI_SOCKET")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        cmd.envs(envs.iter().copied());
        // SAFETY: only async-signal-safe syscalls are used between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        Ok(Self {
            master,
            child: Some(child),
            pending: VecDeque::new(),
            reaped: false,
        })
    }
    pub fn from_external(master: File) -> Self {
        Self {
            master,
            child: None,
            pending: VecDeque::new(),
            reaped: false,
        }
    }
    pub fn child_id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
    pub fn owner_uid(&self) -> io::Result<u32> {
        use std::os::unix::fs::MetadataExt;
        let fd = unsafe {
            libc::ioctl(
                self.master.as_raw_fd(),
                0x5441,
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let slave = unsafe { File::from_raw_fd(fd) };
        Ok(slave.metadata()?.uid())
    }
    pub fn input_allowed(&self, personal: bool) -> bool {
        personal && !self.password_prompt()
    }
    pub fn discard_pending(&mut self) {
        self.pending.clear();
    }
    pub fn request_redraw(&self) {
        let mut pgid = 0;
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCGPGRP, &mut pgid) } == 0
            && pgid > 0
        {
            unsafe {
                libc::kill(-pgid, libc::SIGWINCH);
            }
        }
    }
    /// PTY ownership is changed by login after authentication. Only attach personal
    /// features to the account used for this renderer, never another user's login.
    pub fn user_session(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        let mut path = [0i8; 256];
        if unsafe { libc::ptsname_r(self.master.as_raw_fd(), path.as_mut_ptr(), path.len()) } != 0 {
            return false;
        }
        let path = unsafe { std::ffi::CStr::from_ptr(path.as_ptr()) };
        path.to_str()
            .ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .is_some_and(|m| m.uid() != 0 && m.uid() == unsafe { libc::geteuid() })
    }
    pub fn password_prompt(&self) -> bool {
        let mut modes = unsafe { std::mem::zeroed::<libc::termios>() };
        (unsafe { libc::tcgetattr(self.master.as_raw_fd(), &mut modes) }) != 0
            || (modes.c_lflag & libc::ICANON != 0 && modes.c_lflag & libc::ECHO == 0)
    }
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width: usize,
        cell_height: usize,
    ) -> io::Result<()> {
        let size = libc::winsize {
            ws_col: cols,
            ws_row: rows,
            ws_xpixel: (usize::from(cols) * cell_width).min(u16::MAX as usize) as u16,
            ws_ypixel: (usize::from(rows) * cell_height).min(u16::MAX as usize) as u16,
        };
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.pending.len() + bytes.len() > 4 * 1024 * 1024 {
            return Err(io::Error::other("PTY input queue exceeded 4 MiB"));
        }
        self.pending.extend(bytes);
        self.flush()
    }
    pub fn flush(&mut self) -> io::Result<()> {
        while !self.pending.is_empty() {
            match self.master.write(self.pending.as_slices().0) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "PTY closed")),
                Ok(n) => {
                    self.pending.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    pub fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.master.read(bytes)
    }
    pub fn exited(&mut self) -> io::Result<bool> {
        let Some(child) = self.child.as_mut() else {
            return Ok(false);
        };
        if self.reaped {
            return Ok(true);
        }
        self.reaped = child.try_wait()?.is_some();
        Ok(self.reaped)
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        // Closing the master first hangs up login even after our uid was dropped.
        if let Ok(null) = File::open("/dev/null") {
            drop(std::mem::replace(&mut self.master, null));
        }
        let Some(child) = self.child.as_mut() else {
            return;
        };
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGHUP);
        }
        if !self.reaped {
            let deadline = Instant::now() + std::time::Duration::from_millis(300);
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if child.kill().is_ok() {
                let _ = child.wait();
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_master_does_not_own_or_reap_login() {
        let p = Pty::spawn("/bin/sh", &["-c", "sleep 1"], 80, 24).unwrap();
        let mut frontend = Pty::from_external(p.master.try_clone().unwrap());
        assert!(!frontend.exited().unwrap());
        assert_eq!(frontend.child_id(), None);
        drop(frontend);
        assert!(p.master.metadata().is_ok());
    }
    #[test]
    fn canonical_password_input_disables_ime_but_raw_tui_does_not() {
        let mut p = Pty::spawn(
            "/bin/sh",
            &[
                "-c",
                "stty -echo; printf ready; read answer; stty raw -echo; printf raw; sleep 1",
            ],
            80,
            24,
        )
        .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        let mut out = Vec::new();
        while Instant::now() < deadline && !out.ends_with(b"ready") {
            let mut buf = [0; 128];
            if let Ok(n) = p.read(&mut buf) {
                out.extend_from_slice(&buf[..n]);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(out.ends_with(b"ready"));
        assert!(p.password_prompt());
        assert!(!p.input_allowed(true));
        assert!(!p.input_allowed(false));
        p.send(b"test-secret\n").unwrap();
        while Instant::now() < deadline && !out.ends_with(b"raw") {
            let mut buf = [0; 128];
            if let Ok(n) = p.read(&mut buf) {
                out.extend_from_slice(&buf[..n]);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(out.ends_with(b"raw"));
        assert!(!p.password_prompt());
        assert!(p.input_allowed(true));
        assert!(!String::from_utf8_lossy(&out).contains("test-secret"));
    }
    #[test]
    fn child_has_controlling_terminal_and_dimensions() {
        let mut p = Pty::spawn(
            "/bin/sh",
            &[
                "-c",
                "test -t 0 && stty size && printf '\\033[32m你好\\033[0m'",
            ],
            80,
            24,
        )
        .unwrap();
        let mut out = Vec::new();
        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        while Instant::now() < deadline {
            let mut buf = [0; 4096];
            match p.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(e) => panic!("{e}"),
            }
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("24 80"), "{text}");
        assert!(text.contains("你好"), "{text}");
    }
}
