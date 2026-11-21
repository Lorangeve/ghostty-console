//! Root supervisor for a login-before-shell console. Rendering runs after setuid.
use std::{
    env,
    ffi::{CStr, CString},
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub struct User {
    pub uid: u32,
    pub gid: u32,
    pub name: CString,
    pub home: PathBuf,
}
impl User {
    pub fn lookup(name: &str) -> Result<Self> {
        let name = CString::new(name)?;
        Self::lookup_record(Some(name.as_c_str()), None)
    }
    pub fn lookup_uid(uid: u32) -> Result<Self> {
        Self::lookup_record(None, Some(uid))
    }
    fn lookup_record(name: Option<&CStr>, uid: Option<u32>) -> Result<Self> {
        let mut size = 16384;
        loop {
            let mut storage = vec![0u8; size];
            let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
            let mut result = std::ptr::null_mut();
            let code = unsafe {
                if let Some(name) = name {
                    libc::getpwnam_r(
                        name.as_ptr(),
                        &mut entry,
                        storage.as_mut_ptr().cast(),
                        storage.len(),
                        &mut result,
                    )
                } else {
                    libc::getpwuid_r(
                        uid.unwrap(),
                        &mut entry,
                        storage.as_mut_ptr().cast(),
                        storage.len(),
                        &mut result,
                    )
                }
            };
            if code == libc::ERANGE && size < 1024 * 1024 {
                size *= 2;
                continue;
            }
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code).into());
            }
            if result.is_null() {
                return Err("Unknown renderer user".into());
            }
            if entry.pw_uid == 0 {
                return Err("Renderer must use an unprivileged account".into());
            }
            return Ok(Self {
                uid: entry.pw_uid,
                gid: entry.pw_gid,
                name: unsafe { CStr::from_ptr(entry.pw_name) }.to_owned(),
                home: unsafe { CStr::from_ptr(entry.pw_dir) }.to_str()?.into(),
            });
        }
    }
    pub fn drop_privileges(&self) -> Result<()> {
        if unsafe { libc::initgroups(self.name.as_ptr(), self.gid) } != 0
            || unsafe { libc::setgid(self.gid) } != 0
            || unsafe { libc::setuid(self.uid) } != 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if unsafe { libc::geteuid() } != self.uid || unsafe { libc::getuid() } != self.uid {
            return Err("Privilege drop failed".into());
        }
        // Retain no saved root identity or privilege-gaining exec path.
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
}
pub fn trusted_file(path: &Path) -> Result<()> {
    let meta = fs::metadata(path)?;
    if meta.uid() != 0 || meta.mode() & 0o022 != 0 || !meta.is_file() {
        return Err(format!(
            "Root service needs a root-owned, non-writable file: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}
pub fn launch(name: &str) -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("login-service must run as root".into());
    }
    if crate::console::native_vt(false)? != 6 {
        return Err("Login service is restricted to tty6".into());
    }
    let preferred = User::lookup(name)?;
    let greeter = User::lookup("ghostty-console")?;
    let home = fs::symlink_metadata(&greeter.home)?;
    if greeter.uid == preferred.uid
        || greeter.uid >= 1000
        || greeter.home != Path::new("/var/lib/ghostty-console/greeter")
        || !home.is_dir()
        || home.uid() != 0
        || home.mode() & 0o022 != 0
    {
        return Err("Greeter must be a dedicated system account with a root-owned home".into());
    }
    let config = crate::settings::get()?
        .source
        .as_ref()
        .ok_or("Login service requires --config-file")?
        .clone();
    trusted_file(&config)?;
    let binary = env::current_exe()?;
    trusted_file(&binary)?;
    trusted_file(Path::new(crate::login_gate::SHELL))?;
    crate::session::cleanup_record()?;
    let _restore = crate::console::TtyRestore::save()?;
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        signal_hook::flag::register(sig, stop.clone())?;
    }
    let broker = crate::broker::NativeBroker::new()?;
    let gate = crate::login_gate::Gate::new()?;
    let login = crate::pty::Pty::spawn_login(6, 80, 24)?;
    let mut service = Service {
        broker,
        gate,
        login,
        frontend: None,
        session: None,
        binary,
        config,
    };
    service.spawn_frontend(&greeter, true)?;
    let mut authenticated = false;
    while !stop.load(Ordering::Relaxed) {
        // Capture the PAM scope before reaping login. Even an abandoned scope
        // can contain detached descendants after pam_close_session.
        if service.session.is_none() {
            service.session =
                crate::session::OwnedSession::capture(service.login.child_id().unwrap())?;
            if let Some(session) = &service.session {
                session.persist()?;
            }
        }
        service.broker.tick()?;
        if service.login.exited()? {
            break;
        }
        if !authenticated
            && let Some(session) = service.gate.poll(
                service.login.child_id().unwrap(),
                service.login.owner_uid()?,
            )?
        {
            let uid = session.uid;
            service.session = Some(session);
            authenticated = true;
            if uid != 0 {
                let user = User::lookup_uid(uid)?;
                service.stop_frontend();
                service.spawn_frontend(&user, false)?;
                eprintln!("authenticated renderer uid={uid}; greeter replaced");
            }
        }
        if service
            .frontend
            .as_mut()
            .is_some_and(|p| p.try_wait().ok().flatten().is_some())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Drop terminates only this captured scope; tty1 and user@.service survive.
    Ok(())
}
struct Service {
    broker: crate::broker::NativeBroker,
    gate: crate::login_gate::Gate,
    login: crate::pty::Pty,
    frontend: Option<std::process::Child>,
    session: Option<crate::session::OwnedSession>,
    binary: PathBuf,
    config: PathBuf,
}
impl Service {
    fn spawn_frontend(&mut self, user: &User, greeter: bool) -> Result<()> {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::{net::UnixDatagram, process::CommandExt},
        };
        let (rpc_parent, rpc_child) = UnixDatagram::pair()?;
        let (event_parent, event_child) = UnixDatagram::pair()?;
        // Duplicate above target descriptors to avoid dup2 source collisions.
        let mut inherited = Vec::new();
        for fd in [
            rpc_child.as_raw_fd(),
            event_child.as_raw_fd(),
            self.login.master.as_raw_fd(),
        ] {
            let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 64) };
            if copy < 0 {
                return Err(io::Error::last_os_error().into());
            }
            inherited.push(unsafe { fs::File::from_raw_fd(copy) });
        }
        let source: [i32; 3] = std::array::from_fn(|i| inherited[i].as_raw_fd());
        let name = user.name.to_str()?;
        let mut command = Command::new(&self.binary);
        command
            .arg("--config-file")
            .arg(&self.config)
            .args([
                "login-worker",
                name,
                if greeter { "greeter" } else { "user" },
            ])
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin")
            .env("HOME", &user.home)
            .env("USER", name)
            .env("LOGNAME", name)
            .env("LANG", "en_US.UTF-8")
            .env("XDG_CONFIG_HOME", user.home.join(".config"))
            .env("XDG_DATA_HOME", user.home.join(".local/share"))
            .env("XDG_STATE_HOME", user.home.join(".local/state"))
            .env("GHOSTTY_CONSOLE_BROKER", "1")
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if greeter {
            command
                .env("FONTCONFIG_FILE", "/etc/fonts/fonts.conf")
                .env("FONTCONFIG_PATH", "/etc/fonts")
                .env("XDG_CONFIG_HOME", "/etc/ghostty-console/greeter")
                .env("XDG_DATA_HOME", "/usr/share/ghostty-console/greeter")
                .env("XDG_CACHE_HOME", "/var/cache/fontconfig");
        } else {
            command.env("XDG_RUNTIME_DIR", format!("/run/user/{}", user.uid));
        }
        unsafe {
            command.pre_exec(move || {
                for (index, fd) in source.into_iter().enumerate() {
                    if libc::dup2(fd, 3 + index as i32) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        drop(inherited);
        drop(rpc_child);
        drop(event_child);
        self.frontend = Some(child);
        self.broker.attach(rpc_parent, event_parent)?;
        Ok(())
    }
    fn stop_frontend(&mut self) {
        if let Some(child) = self.frontend.as_mut() {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                let _ = self.broker.tick();
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        self.frontend = None;
        self.broker.detach();
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop_frontend();
        if self.session.is_none()
            && let Some(pid) = self.login.child_id()
        {
            self.session = crate::session::OwnedSession::capture(pid).ok().flatten();
        }
        if let Some(session) = &self.session {
            if let Err(error) = session.terminate() {
                eprintln!("session {} cleanup failed: {error}", session.id);
            } else {
                let _ = fs::remove_file(crate::session::RECORD);
            }
        }
    }
}
