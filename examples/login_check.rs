//! Noninteractive root smoke test: reach login's password prompt without supplying credentials.
use ghostty_console::pty::Pty;
use std::{
    io,
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("Run this isolated login PTY check as root".into());
    }
    let mut pty = Pty::spawn_login(6, 80, 24)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut prompt = false;
    while Instant::now() < deadline {
        let mut bytes = [0; 4096];
        match pty.read(&mut bytes) {
            Ok(n) if n > 0 => {
                if String::from_utf8_lossy(&bytes[..n]).contains("login:") {
                    prompt = true;
                    break;
                }
            }
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => return Err(e.into()),
            _ => (),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !prompt {
        return Err("login did not produce its username prompt".into());
    }
    pty.send(b"lorangeve\n")?;
    while Instant::now() < deadline && !pty.password_prompt() {
        let mut bytes = [0; 4096];
        let _ = pty.read(&mut bytes);
        std::thread::sleep(Duration::from_millis(10));
    }
    if !pty.password_prompt() {
        return Err("Password prompt did not disable echo".into());
    }
    if pty.user_session() {
        return Err("Unauthenticated PTY was treated as logged in".into());
    }
    println!(
        "System login username/password prompts: OK; password echo disabled; no credentials supplied."
    );
    Ok(())
}
