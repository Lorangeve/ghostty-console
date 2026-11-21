//! Track only the PAM scope belonging to our own login child, including after
//! logind abandons it. Never terminate a user slice or another VT's session.
use std::{
    ffi::{CStr, CString},
    io,
    time::{Duration, Instant},
};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::OwnedObjectPath,
};
#[link(name = "systemd")]
unsafe extern "C" {
    fn sd_pid_get_session(pid: libc::pid_t, session: *mut *mut libc::c_char) -> i32;
    fn sd_session_get_leader(session: *const libc::c_char, leader: *mut libc::pid_t) -> i32;
    fn sd_session_get_uid(session: *const libc::c_char, uid: *mut libc::uid_t) -> i32;
}
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub struct OwnedSession {
    pub id: String,
    pub uid: u32,
    scope: String,
    invocation: Vec<u8>,
}
fn bus() -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::system()?
        .method_timeout(Duration::from_secs(1))
        .build()
}
fn unit_path(conn: &Connection, scope: &str) -> zbus::Result<OwnedObjectPath> {
    Proxy::new(
        conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )?
    .call("GetUnit", &(scope,))
}
impl OwnedSession {
    pub fn capture(pid: u32) -> Result<Option<Self>> {
        Self::capture_for(pid, pid)
    }
    pub fn capture_for(pid: u32, expected_leader: u32) -> Result<Option<Self>> {
        let mut raw = std::ptr::null_mut();
        let result = unsafe { sd_pid_get_session(pid as _, &mut raw) };
        if result < 0 {
            return Ok(None);
        }
        let id = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        unsafe {
            libc::free(raw.cast());
        }
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("Invalid PAM session id".into());
        }
        let name = CString::new(id.clone())?;
        let (mut leader, mut uid) = (0, 0);
        if unsafe { sd_session_get_leader(name.as_ptr(), &mut leader) } < 0
            || leader != expected_leader as i32
        {
            return Ok(None);
        }
        let r = unsafe { sd_session_get_uid(name.as_ptr(), &mut uid) };
        if r < 0 {
            return Err(io::Error::from_raw_os_error(-r).into());
        }
        let scope = format!("session-{id}.scope");
        let conn = bus()?;
        let path = unit_path(&conn, &scope)?;
        let unit = Proxy::new(
            &conn,
            "org.freedesktop.systemd1",
            path.as_str(),
            "org.freedesktop.systemd1.Unit",
        )?;
        let invocation: Vec<u8> = unit.get_property("InvocationID")?;
        if invocation.len() != 16 {
            return Err("Missing session scope invocation identity".into());
        }
        Ok(Some(Self {
            id,
            uid,
            scope,
            invocation,
        }))
    }
    pub fn terminate(&self) -> Result<()> {
        let conn = bus()?;
        let path = match unit_path(&conn, &self.scope) {
            Ok(p) => p,
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "org.freedesktop.systemd1.NoSuchUnit" =>
            {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let unit = Proxy::new(
            &conn,
            "org.freedesktop.systemd1",
            path.as_str(),
            "org.freedesktop.systemd1.Unit",
        )?;
        if unit.get_property::<Vec<u8>>("InvocationID")? != self.invocation {
            return Err("Session scope identity changed; refusing termination".into());
        }
        let manager = Proxy::new(
            &conn,
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
        )?;
        let _: OwnedObjectPath = manager.call("StopUnit", &(&self.scope, "replace"))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match unit.get_property::<String>("ActiveState") {
                Ok(s) if s == "inactive" || s == "failed" => return Ok(()),
                Err(zbus::Error::MethodError(name, _, _))
                    if matches!(
                        name.as_str(),
                        "org.freedesktop.DBus.Error.UnknownObject"
                            | "org.freedesktop.systemd1.NoSuchUnit"
                    ) =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
                _ => (),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Scope stop defaults can be much longer than our service timeout. Send
        // SIGKILL only to this verified invocation if graceful termination stalled.
        if unit.get_property::<Vec<u8>>("InvocationID")? == self.invocation {
            let _: () = manager.call("KillUnit", &(&self.scope, "all", libc::SIGKILL))?;
        }
        Ok(())
    }
}

pub const RECORD: &str = "/run/ghostty-console/tty6-session";
impl OwnedSession {
    pub fn persist(&self) -> Result<()> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        let dir = std::path::Path::new(RECORD).parent().unwrap();
        std::fs::create_dir_all(dir)?;
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err("Untrusted session record directory".into());
        }
        let temp = dir.join(format!(".session-{}", std::process::id()));
        let text = format!(
            "{}\n{}\n{}\n{}\n",
            self.id,
            self.uid,
            self.scope,
            self.invocation
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        use std::io::Write;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(temp, RECORD)?;
        Ok(())
    }
    fn from_record(text: &str) -> Result<Self> {
        let lines: Vec<_> = text.lines().collect();
        if lines.len() != 4
            || lines[0].is_empty()
            || !lines[0]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || lines[2] != format!("session-{}.scope", lines[0])
            || lines[3].len() != 32
            || !lines[3].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("Invalid session cleanup record".into());
        }
        let invocation = (0..32)
            .step_by(2)
            .map(|i| u8::from_str_radix(&lines[3][i..i + 2], 16))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Self {
            id: lines[0].into(),
            uid: lines[1].parse()?,
            scope: lines[2].into(),
            invocation,
        })
    }
}
pub fn cleanup_record() -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if unsafe { libc::geteuid() } != 0 {
        return Err("login-cleanup requires root".into());
    }
    let meta = match std::fs::symlink_metadata(RECORD) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o022 != 0 || meta.len() > 1024 {
        return Err("Untrusted session cleanup record".into());
    }
    let session = OwnedSession::from_record(&std::fs::read_to_string(RECORD)?)?;
    session.terminate()?;
    std::fs::remove_file(RECORD)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_record_cannot_name_an_unrelated_unit() {
        for text in [
            "3\n1000\nuser@1000.service\n00000000000000000000000000000000\n",
            "../x\n1000\nsession-../x.scope\n00000000000000000000000000000000\n",
            "3\n1000\nsession-3.scope\nwrong\n",
        ] {
            assert!(OwnedSession::from_record(text).is_err());
        }
        assert!(
            OwnedSession::from_record(
                "19\n1000\nsession-19.scope\n00000000000000000000000000000001\n"
            )
            .is_ok()
        );
    }
}
