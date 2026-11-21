//! Privileged isolated PAM scope check; no password and no changes to live VTs.
use ghostty_console::session::OwnedSession;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("Root is required for the isolated PAM scope test".into());
    }
    let user = std::env::args()
        .nth(1)
        .ok_or("Pass an unprivileged test account")?;
    let script = r#"import os,signal,time
pid=os.fork()
if pid==0:
 os.setsid()
 signal.signal(signal.SIGHUP,signal.SIG_IGN)
 signal.signal(signal.SIGTERM,signal.SIG_IGN)
 fd=os.open('/dev/null',os.O_RDWR)
 for target in (0,1,2): os.dup2(fd,target)
 time.sleep(15)
 os._exit(0)
print(pid,flush=True)
input()
"#;
    let mut child = Command::new("/usr/bin/runuser")
        .args(["-u", &user, "--", "/usr/bin/python3", "-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line)?;
    let detached: i32 = line.trim().parse()?;
    struct Cleanup(i32);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
            }
        }
    }
    let _cleanup = Cleanup(detached);
    let deadline = Instant::now() + Duration::from_secs(3);
    let session = loop {
        if let Some(s) = OwnedSession::capture(child.id())? {
            break s;
        }
        if Instant::now() > deadline {
            return Err("PAM scope was not captured".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    child.stdin.take().unwrap().write_all(b"exit\n")?;
    child.wait()?;
    if unsafe { libc::kill(detached, 0) } != 0 {
        return Err("Test descendant disappeared before scope cleanup".into());
    }
    println!(
        "Captured own PAM scope {}; detached descendant survived parent exit",
        session.id
    );
    session.terminate()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let status =
            std::fs::read_to_string(format!("/proc/{detached}/status")).unwrap_or_default();
        if status.is_empty()
            || status
                .lines()
                .any(|l| l.starts_with("State:") && l.contains('Z'))
        {
            break;
        }
        if Instant::now() > deadline {
            return Err("Detached descendant survived verified scope cleanup".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("Verified scope cleanup killed the detached descendant; unrelated sessions untouched");
    Ok(())
}
