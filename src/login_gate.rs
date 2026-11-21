//! Authenticated shell barrier: user code starts only after its PAM scope has
//! been persisted for ExecStopPost cleanup. Peers are verified with SO_PEERCRED
//! and the scope's kernel/logind leader, never a claimed username or session id.
use std::{
    fs,
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::Path,
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub const SOCKET: &str = "/run/ghostty-console/login-gate";
pub const SHELL: &str = "/usr/local/libexec/ghostty-console-login-shell";
fn credentials(stream: &UnixStream) -> io::Result<libc::ucred> {
    let mut c: libc::ucred = unsafe { std::mem::zeroed() };
    let mut n = std::mem::size_of_val(&c) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut c as *mut libc::ucred).cast(),
            &mut n,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(c)
}
pub struct Gate(UnixListener);
impl Gate {
    pub fn new() -> Result<Self> {
        let dir = Path::new(SOCKET).parent().unwrap();
        fs::create_dir_all(dir)?;
        let meta = fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err("Untrusted shell gate directory".into());
        }
        match fs::symlink_metadata(SOCKET) {
            Ok(m) if m.uid() == 0 && m.file_type().is_socket() => fs::remove_file(SOCKET)?,
            Ok(_) => return Err("Untrusted old shell gate".into()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let socket = UnixListener::bind(SOCKET)?;
        fs::set_permissions(SOCKET, fs::Permissions::from_mode(0o666))?;
        socket.set_nonblocking(true)?;
        Ok(Self(socket))
    }
    pub fn poll(&self, login_pid: u32, owner: u32) -> Result<Option<crate::session::OwnedSession>> {
        for _ in 0..16 {
            let (mut stream, _) = match self.0.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            };
            stream.set_nonblocking(true)?;
            let peer = credentials(&stream)?;
            if peer.uid != owner {
                continue;
            }
            // A different user's existing desktop session cannot acknowledge this login.
            if let Some(session) =
                crate::session::OwnedSession::capture_for(peer.pid as u32, login_pid)?
            {
                if session.uid != peer.uid {
                    continue;
                }
                session.persist()?;
                stream.write_all(b"Y")?;
                return Ok(Some(session));
            }
        }
        Ok(None)
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        let _ = fs::remove_file(SOCKET);
    }
}
pub fn shell() -> Result<()> {
    let mut stream = UnixStream::connect(SOCKET)?;
    if credentials(&stream)?.uid != 0 {
        return Err("Shell gate is not owned by root".into());
    }
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut ack = [0; 1];
    stream.read_exact(&mut ack)?;
    if ack != *b"Y" {
        return Err("Login session was not registered".into());
    }
    drop(stream);
    // The original account shell remains authoritative, including nologin/false.
    let mut size = 16384;
    let shell = loop {
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found = std::ptr::null_mut();
        let mut buf = vec![0u8; size];
        let code = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut entry,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut found,
            )
        };
        if code == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if code != 0 || found.is_null() {
            return Err("Cannot resolve authenticated account shell".into());
        }
        let name = unsafe { std::ffi::CStr::from_ptr(entry.pw_shell) }
            .to_str()?
            .to_owned();
        break if name.is_empty() {
            "/bin/sh".into()
        } else {
            name
        };
    };
    if !shell.starts_with('/') || shell == SHELL {
        return Err("Invalid account shell".into());
    }
    let name = Path::new(&shell).file_name().unwrap().to_string_lossy();
    Err(std::process::Command::new(&shell)
        .arg0(format!("-{name}"))
        .exec()
        .into())
}
