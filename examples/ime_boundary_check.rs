//! Local fixture check: no Lua in the greeter; locked Rime remains nonfatal.
use ghostty_console::{ime::LazyIme, settings};
use std::{fs::OpenOptions, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("Pass a fixture config and its Lua marker path".into());
    }
    let settings = settings::initialize(Some(Path::new(&args[0])))?;
    if Path::new(&args[1]).exists() {
        return Err("Remove the fixture marker before this check".into());
    }
    let mut ime = LazyIme::default();
    assert!(ime.activate(false).is_err());
    assert!(!ime.ready());
    assert!(!Path::new(&args[1]).exists());
    let user = settings
        .rime_user_dir
        .as_ref()
        .ok_or("Fixture must set rime-user-dir")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(user.join(".ghostty-console.lock"))?;
    lock.try_lock()?;
    assert!(ime.activate(true).is_err());
    assert!(!ime.ready());
    ime.clear();
    assert!(!Path::new(&args[1]).exists());
    println!(
        "Greeter did not execute Lua; locked personal Rime was rejected without terminating the console path"
    );
    Ok(())
}
