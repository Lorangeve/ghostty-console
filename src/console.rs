use crate::{
    clipboard::Clipboard,
    config,
    display::{Display, SharedSeat},
    interaction::Selection,
    keyboard::{Keyboard, PointerEvent},
    pty::Pty,
    renderer::Renderer,
    terminal::Terminal,
};
use std::{
    cell::RefCell,
    collections::{HashSet, VecDeque},
    env,
    fs::{self, File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::PathBuf,
    process::{Command, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const KDGETMODE: libc::c_ulong = 0x4b3b;
const KDSETMODE: libc::c_ulong = 0x4b3a;
const KDGKBMODE: libc::c_ulong = 0x4b44;
const KDSKBMODE: libc::c_ulong = 0x4b45;

struct Options {
    timeout: u64,
    device: PathBuf,
    check: bool,
}
fn options(args: &[String]) -> Result<Options> {
    let settings = crate::settings::get()?;
    let mut timeout = settings.timeout;
    let mut device = settings.drm_device.clone();
    let mut check = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--timeout" => timeout = iter.next().ok_or("Missing --timeout value")?.parse()?,
            "--device" => {
                device = Some(PathBuf::from(iter.next().ok_or("Missing --device value")?))
            }
            "--check" => check = true,
            _ => return Err(format!("Unknown console option: {arg}").into()),
        }
    }
    let device = match device {
        Some(p) => p,
        None => {
            let mut cards: Vec<PathBuf> = fs::read_dir("/dev/dri")?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .strip_prefix("card")
                        .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
                })
                .collect();
            cards.sort();
            cards.into_iter().next().ok_or("No DRM card found")?
        }
    };
    Ok(Options {
        timeout,
        device,
        check,
    })
}
pub(crate) fn native_vt(require_active: bool) -> Result<u32> {
    let metadata = fs::metadata("/proc/self/fd/0")?;
    let major = libc::major(metadata.rdev());
    let minor = libc::minor(metadata.rdev());
    if major != 4 || !(1..=63).contains(&minor) {
        return Err("con requires a native /dev/ttyN login. Refusing a desktop PTY or kmscon shell; run con --check for prerequisites.".into());
    }
    let active = fs::read_to_string("/sys/class/tty/tty0/active")?;
    if require_active && active.trim() != format!("tty{minor}") {
        return Err("Run con from the currently active VT".into());
    }
    if minor == 1 {
        return Err(
            "tty1 is reserved for the existing desktop; use a native getty on another VT".into(),
        );
    }
    Ok(minor)
}
pub(crate) struct TtyRestore {
    file: File,
    kd: i32,
    kb: i32,
    termios: libc::termios,
}
impl TtyRestore {
    pub(crate) fn save() -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let fd = file.as_raw_fd();
        let mut kd = 0;
        let mut kb = 0;
        let mut termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(fd, KDGETMODE, &mut kd) } < 0
            || unsafe { libc::ioctl(fd, KDGKBMODE, &mut kb) } < 0
            || unsafe { libc::tcgetattr(fd, &mut termios) } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self {
            file,
            kd,
            kb,
            termios,
        })
    }
    fn graphics(&self) -> Result<()> {
        let fd = self.file.as_raw_fd();
        let mut raw = self.termios;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } < 0
            || unsafe { libc::ioctl(fd, KDSKBMODE, 4) } < 0
            || unsafe { libc::ioctl(fd, KDSETMODE, 1) } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
}
impl Drop for TtyRestore {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        unsafe {
            libc::ioctl(fd, KDSKBMODE, self.kb);
            libc::ioctl(fd, KDSETMODE, self.kd);
            libc::tcsetattr(fd, libc::TCSANOW, &self.termios);
        }
    }
}
fn stop_flag() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    Ok(stop)
}
pub fn launch(args: &[String]) -> Result<()> {
    let opts = options(args)?;
    if opts.check {
        println!(
            "DRM device: {}\nActive VT: {}\nStdin: {}",
            opts.device.display(),
            fs::read_to_string("/sys/class/tty/tty0/active")?.trim(),
            fs::read_link("/proc/self/fd/0")?.display()
        );
        match native_vt(true) {
            Ok(vt) => println!("Native tty{vt}: OK"),
            Err(e) => println!("Native VT check: {e}"),
        }
        let _renderer = Renderer::new()?;
        println!("Fonts: OK");
        let _rime = config::open_rime(None)?;
        println!("Rime: OK");
        let mut term = Terminal::new(80, 24)?;
        term.write("你好".as_bytes());
        term.frame()?;
        println!("libghostty-vt: OK\nNo device was taken over.");
        return Ok(());
    }
    let vt = native_vt(true)?;
    let restore = TtyRestore::save()?;
    let state = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap()).join(".local/state"))
        .join("ghostty-console");
    fs::create_dir_all(&state)?;
    let log_path = state.join("con.log");
    let log = File::create(&log_path)?;
    println!(
        "Starting on tty{vt}; Ctrl+Alt+Esc exits. Timeout: {}s. Log: {}",
        opts.timeout,
        log_path.display()
    );
    let stop = stop_flag()?;
    let mut worker = Command::new(env::current_exe()?);
    if let Some(source) = &crate::settings::get()?.source {
        worker.env("GHOSTTY_CONSOLE_CONFIG", source);
    }
    let mut child = worker
        .arg("con-worker")
        .args(args)
        .env("LIBSEAT_BACKEND", "logind")
        .stdin(Stdio::inherit())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    let started = Instant::now();
    let mut terminating = None;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let expired = opts.timeout > 0 && started.elapsed() >= Duration::from_secs(opts.timeout);
        if (expired || stop.load(Ordering::Relaxed)) && terminating.is_none() {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            terminating = Some(Instant::now());
        }
        if terminating.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
            let _ = child.kill();
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    drop(restore);
    if !status.success() {
        return Err(format!("Console exited ({status}). Log: {}", log_path.display()).into());
    }
    println!("Returned to tty{vt}. Log: {}", log_path.display());
    Ok(())
}

pub fn worker(args: &[String]) -> Result<()> {
    worker_inner(args, None, false)
}
pub fn login_worker(name: &str, greeter: bool) -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("login-worker requires root startup".into());
    }
    worker_inner(
        &[],
        Some(crate::login_service::User::lookup(name)?),
        greeter,
    )
}
fn worker_inner(
    args: &[String],
    login_user: Option<crate::login_service::User>,
    greeter: bool,
) -> Result<()> {
    let login_mode = login_user.is_some();
    let opts = options(args)?;
    eprintln!(
        "configuration: {}",
        crate::settings::get()?
            .source
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "built-in defaults".into())
    );
    let vt = native_vt(!login_mode)?;
    let stop = stop_flag()?;
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let callback_queue = Rc::clone(&queue);
    let callback = move |event| callback_queue.borrow_mut().push_back(event);
    let seat: SharedSeat = Rc::new(RefCell::new(if login_mode {
        if env::var("GHOSTTY_CONSOLE_BROKER").as_deref() != Ok("1") {
            return Err("Private login broker is missing".into());
        }
        use std::os::{fd::FromRawFd, unix::net::UnixDatagram};
        // Only inherited descriptors from the root-owned service. Helpers and
        // user processes must never inherit the control channel or PTY master.
        for fd in [3, 4, 5] {
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error().into());
            }
        }
        crate::seat::Seat::native(
            unsafe { UnixDatagram::from_raw_fd(3) },
            unsafe { UnixDatagram::from_raw_fd(4) },
            callback,
        )?
    } else {
        crate::seat::Seat::logind(callback)?
    }));
    let tty = if login_mode {
        None
    } else {
        Some(TtyRestore::save()?)
    };
    let mut pty = if login_mode {
        use std::os::fd::FromRawFd;
        Some(Pty::from_external(unsafe { File::from_raw_fd(5) }))
    } else {
        None
    };
    if let Some(user) = login_user {
        // The native VT belongs exclusively to the root broker. A personal Lua
        // extension must not inherit a controlling VT or an fd that can change
        // its VT_PROCESS owner. Only the login PTY and private RPC remain.
        if unsafe { libc::setsid() } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let null = File::open("/dev/null")?;
        if unsafe { libc::dup2(null.as_raw_fd(), 0) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        user.drop_privileges()?;
    }
    eprintln!("frontend uid={} login_mode={login_mode}", unsafe {
        libc::geteuid()
    });
    let mut renderer = if greeter {
        Renderer::greeter()?
    } else {
        Renderer::new().or_else(|error| {
            eprintln!("Personal fonts unavailable; using system fonts: {error}");
            Renderer::greeter()
        })?
    };
    renderer.login_screen = greeter;
    let mut rime = crate::ime::LazyIme::default();
    let mut display: Option<Display> = None;
    let mut pending_enable = false;
    let mut next_display_attempt = Instant::now();
    let mut keyboard: Option<Keyboard> = None;
    let mut terminal = if login_mode {
        let mut t = Terminal::new(80, 24)?;
        t.set_cell_size(renderer.cell_width(), renderer.cell_height())?;
        Some(t)
    } else {
        None
    };
    let mut display_geometry = None;
    let settings = crate::settings::get()?;
    let mut enabled = settings.ime_enabled && !greeter;
    let mut overlay = None;
    let mut dirty = true;
    let mut suppressed = HashSet::new();
    let mut ime_consumed = HashSet::new();
    let mut buffer = [0u8; 65536];
    let mut mouse = (0usize, 0usize);
    let mut held_button = 0;
    let mut selecting = false;
    let mut clipboard = if greeter {
        Clipboard::local_only()
    } else {
        Clipboard::new()
    };
    let mut notifications = None;
    let mut was_input_safe = false;
    if !greeter {
        renderer.buffers.push(crate::buffers::Message::new(
            "con",
            "已就绪 · Ctrl+Alt+N 查看 *Messages*",
        ));
    }
    while !stop.load(Ordering::Relaxed) {
        let features = !greeter;
        let mut input_safe = pty.as_ref().is_some_and(|p| p.input_allowed(features));
        if was_input_safe && !input_safe {
            rime.clear();
            overlay = None;
            renderer.selection = None;
            ime_consumed.clear();
            clipboard.invalidate();
            if let Some(p) = pty.as_mut() {
                p.discard_pending();
            }
            dirty = true;
        }
        if renderer.login_screen == features {
            renderer.login_screen = !features;
            dirty = true;
        }
        was_input_safe = input_safe;
        if features && settings.desktop_notifications && notifications.is_none() {
            notifications = Some(crate::notifications::spawn());
        }
        for message in notifications.iter().flat_map(|rx| rx.try_iter()).take(128) {
            renderer.buffers.push(message);
            dirty = true;
        }
        for message in clipboard.messages.try_iter() {
            renderer.buffers.push(message);
            dirty = true;
        }
        dirty |= renderer.buffers.tick();
        seat.borrow_mut().dispatch(0)?;
        loop {
            let event = queue.borrow_mut().pop_front();
            let Some(event) = event else {
                break;
            };
            match event {
                libseat::SeatEvent::Disable => {
                    pending_enable = false;
                    suppressed.clear();
                    selecting = false;
                    held_button = 0;
                    ime_consumed.clear();
                    rime.clear();
                    overlay = None;
                    if let Some(k) = keyboard.as_mut() {
                        k.suspend();
                    }
                    display.take();
                    seat.borrow_mut().disable()?;
                    eprintln!("seat disabled; device access released");
                }
                libseat::SeatEvent::Enable => {
                    pending_enable = true;
                    next_display_attempt = Instant::now();
                }
            }
        }
        if pending_enable
            && Instant::now() >= next_display_attempt
            && (!login_mode
                || fs::read_to_string("/sys/class/tty/tty0/active")?.trim() == format!("tty{vt}"))
        {
            next_display_attempt = Instant::now() + Duration::from_millis(30);
            match Display::open(Rc::clone(&seat), &opts.device) {
                Ok(d) => {
                    let cols = (d.width / renderer.cell_width()).min(500) as u16;
                    let rows = (d.height / renderer.cell_height())
                        .saturating_sub(renderer.panel_rows(enabled, overlay.as_ref()))
                        .min(300) as u16;
                    if cols < 20 || rows < 5 {
                        return Err("Display too small".into());
                    }
                    if display_geometry.is_some_and(|old| old != (d.width, d.height)) {
                        return Err(
                            "Display geometry changed; restart con to use the new mode".into()
                        );
                    }
                    if terminal.is_none() {
                        let mut t = Terminal::new(cols, rows)?;
                        t.set_cell_size(renderer.cell_width(), renderer.cell_height())?;
                        terminal = Some(t);
                        if !login_mode {
                            let arguments: Vec<&str> =
                                settings.command[1..].iter().map(String::as_str).collect();
                            let bus_address = crate::notifications::session_address();
                            let mut p = Pty::spawn_env(
                                &settings.command[0],
                                &arguments,
                                cols,
                                rows,
                                &[("DBUS_SESSION_BUS_ADDRESS", &bus_address)],
                            )?;
                            p.resize(cols, rows, renderer.cell_width(), renderer.cell_height())?;
                            pty = Some(p);
                        } else {
                            pty.as_mut().unwrap().resize(
                                cols,
                                rows,
                                renderer.cell_width(),
                                renderer.cell_height(),
                            )?;
                        }
                        display_geometry = Some((d.width, d.height));
                    }
                    if login_mode {
                        terminal.as_mut().unwrap().resize(cols, rows)?;
                        pty.as_mut().unwrap().resize(
                            cols,
                            rows,
                            renderer.cell_width(),
                            renderer.cell_height(),
                        )?;
                        display_geometry = Some((d.width, d.height));
                        pty.as_ref().unwrap().request_redraw();
                    }
                    if let Some(k) = keyboard.as_mut() {
                        k.resume()?;
                    } else {
                        keyboard = Some(Keyboard::new(Rc::clone(&seat))?);
                    }
                    if let Some(tty) = &tty {
                        tty.graphics()?;
                    }
                    eprintln!("seat enabled: {}x{}, grid {cols}x{rows}", d.width, d.height);
                    display = Some(d);
                    dirty = true;
                    pending_enable = false;
                }
                Err(error)
                    if error
                        .downcast_ref::<io::Error>()
                        .is_some_and(crate::display::device_transition) => {}
                Err(error) => return Err(error),
            }
        }
        if let (Some(t), Some(p)) = (terminal.as_mut(), pty.as_mut()) {
            // Bound each batch so a noisy child cannot starve seat disable events.
            for _ in 0..4 {
                match p.read(&mut buffer) {
                    Ok(0) => return Ok(()),
                    Ok(n) => {
                        if !selecting {
                            renderer.selection = None;
                        }
                        t.write(&buffer[..n]);
                        dirty = true;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.raw_os_error() == Some(libc::EIO) => return Ok(()),
                    Err(e) => return Err(e.into()),
                }
            }
            p.send(&t.take_replies())?;
            if let Some(text) = t.take_clipboard()
                && p.input_allowed(features)
            {
                clipboard.copy(text);
            }
            p.flush()?;
            if p.exited()? {
                return Ok(());
            }
        }
        input_safe = pty.as_ref().is_some_and(|p| p.input_allowed(features));
        if enabled
            && input_safe
            && !rime.ready()
            && let Err(error) = rime.activate(features)
        {
            enabled = false;
            renderer.buffers.push(crate::buffers::Message::new(
                "输入法",
                &format!("Rime 不可用，保持英文输入: {error}"),
            ));
            dirty = true;
        }
        if let Some(d) = display.as_ref() {
            let batch = keyboard.as_mut().unwrap().events()?;
            for key in batch.keys {
                input_safe = pty.as_ref().is_some_and(|p| p.input_allowed(features));
                if !input_safe {
                    rime.clear();
                    overlay = None;
                    ime_consumed.clear();
                    renderer.selection = None;
                    clipboard.invalidate();
                    pty.as_mut().unwrap().discard_pending();
                }
                if suppressed.contains(&key.code) {
                    if !key.pressed {
                        suppressed.remove(&key.code);
                    }
                    continue;
                }
                if key.pressed && key.mods & 6 == 6 {
                    if key.code == 23 && !key.repeated {
                        renderer.panel_visible = !renderer.panel_visible;
                        renderer.selection = None;
                        selecting = false;
                        dirty = true;
                        suppressed.insert(key.code);
                        eprintln!("panel visible={}", renderer.panel_visible);
                        continue;
                    }
                    if key.code == 49 && !key.repeated && features {
                        renderer.buffers.toggle_messages();
                        renderer.selection = None;
                        selecting = false;
                        renderer.help_visible = false;
                        dirty = true;
                        suppressed.insert(key.code);
                        continue;
                    }
                    if key.code == 50 && !key.repeated {
                        renderer.ime_display.toggle();
                        renderer.selection = None;
                        dirty = true;
                        suppressed.insert(key.code);
                        continue;
                    }
                    if key.code == 35 && !key.repeated {
                        renderer.help_visible = !renderer.help_visible;
                        renderer.buffers.window.buffer = crate::buffers::BufferId::Terminal;
                        dirty = true;
                        suppressed.insert(key.code);
                        continue;
                    }
                    if key.code == 1 {
                        return Ok(());
                    }
                    if let Some(vt) = vt_key(key.code) {
                        suppressed.insert(key.code);
                        seat.borrow_mut().switch_session(vt)?;
                        // Dispatch the seat handshake before processing any more input.
                        break;
                    }
                }
                if renderer.buffers.window.buffer == crate::buffers::BufferId::Messages {
                    if matches!(key.code, 29 | 97 | 42 | 54 | 56 | 100 | 125 | 126) {
                        if !key.pressed {
                            pty.as_mut().unwrap().send(&terminal.as_mut().unwrap().key(
                                key.code,
                                0,
                                key.mods,
                                &key.text,
                                key.unshifted,
                            )?)?;
                        }
                        continue;
                    }
                    if key.pressed {
                        match key.code {
                            1 | 16 => {
                                renderer.buffers.window.buffer = crate::buffers::BufferId::Terminal
                            }
                            104 | 103 => {
                                renderer
                                    .buffers
                                    .scroll(if key.code == 104 { 10 } else { 1 })
                            }
                            109 | 108 => {
                                renderer
                                    .buffers
                                    .scroll(if key.code == 109 { -10 } else { -1 })
                            }
                            105 => {
                                renderer.buffers.window.horizontal =
                                    renderer.buffers.window.horizontal.saturating_sub(8)
                            }
                            106 => {
                                renderer.buffers.window.horizontal =
                                    (renderer.buffers.window.horizontal + 8).min(8192)
                            }
                            102 => renderer.buffers.scroll(isize::MAX),
                            107 => renderer.buffers.window.scroll = 0,
                            _ => (),
                        }
                        suppressed.insert(key.code);
                        dirty = true;
                    }
                    continue;
                }
                if renderer.help_visible {
                    if matches!(key.code, 29 | 97 | 42 | 54 | 56 | 100 | 125 | 126) {
                        if !key.pressed {
                            pty.as_mut().unwrap().send(&terminal.as_mut().unwrap().key(
                                key.code,
                                0,
                                key.mods,
                                &key.text,
                                key.unshifted,
                            )?)?;
                        }
                        continue;
                    }
                    if key.pressed {
                        renderer.help_visible = false;
                        dirty = true;
                        suppressed.insert(key.code);
                    }
                    continue;
                }
                if key.pressed && !key.repeated && key.code == 57 && key.mods & 6 == 2 {
                    if !input_safe {
                        suppressed.insert(key.code);
                        continue;
                    }
                    enabled = !enabled;
                    if enabled && let Err(error) = rime.activate(features) {
                        enabled = false;
                        renderer.buffers.push(crate::buffers::Message::new(
                            "输入法",
                            &format!("Rime 不可用，保持英文输入: {error}"),
                        ));
                    }
                    rime.clear();
                    overlay = None;
                    dirty = true;
                    suppressed.insert(key.code);
                    continue;
                }
                if key.pressed && key.mods & 7 == 3 && (key.code == 46 || key.code == 47) {
                    if !input_safe {
                        suppressed.insert(key.code);
                        continue;
                    }
                    if key.code == 46 {
                        if let Some(selection) = renderer.selection {
                            clipboard.copy(selection.text(&terminal.as_mut().unwrap().frame()?));
                        }
                    } else {
                        clipboard.paste();
                    }
                    suppressed.insert(key.code);
                    continue;
                }
                let t = terminal.as_mut().unwrap();
                let p = pty.as_mut().unwrap();
                let handled = if enabled
                    && p.input_allowed(features)
                    && let Some(update) = rime.process_key(key.keysym as i32, key.rime_mods)
                {
                    p.send(update.commit.as_bytes())?;
                    let handled = update.handled;
                    overlay = Some(update);
                    dirty = true;
                    handled
                } else {
                    false
                };
                if handled && key.pressed {
                    ime_consumed.insert(key.code);
                }
                let consumed_release = !key.pressed && ime_consumed.remove(&key.code);
                if !handled && !consumed_release {
                    p.send(&t.key(
                        key.code,
                        if key.repeated {
                            2
                        } else if key.pressed {
                            1
                        } else {
                            0
                        },
                        key.mods,
                        &key.text,
                        key.unshifted,
                    )?)?;
                }
            }
            for event in batch.pointers {
                input_safe = pty.as_ref().is_some_and(|p| p.input_allowed(features));
                if !input_safe {
                    continue;
                }
                if renderer.buffers.window.buffer == crate::buffers::BufferId::Messages {
                    if let PointerEvent::Wheel { delta, .. } = event {
                        renderer.buffers.scroll(delta as isize * 3);
                        dirty = true;
                    }
                    continue;
                }
                if renderer.help_visible {
                    if let PointerEvent::Motion { dx, dy, .. } = event {
                        mouse.0 = (mouse.0 as i64 + i64::from(dx))
                            .clamp(0, d.width.saturating_sub(1) as i64)
                            as usize;
                        mouse.1 = (mouse.1 as i64 + i64::from(dy))
                            .clamp(0, d.height.saturating_sub(1) as i64)
                            as usize;
                        renderer.pointer = Some(mouse);
                        dirty = true;
                    }
                    if matches!(event, PointerEvent::Button { pressed: true, .. }) {
                        renderer.help_visible = false;
                        dirty = true;
                    }
                    continue;
                }
                let t = terminal.as_mut().unwrap();
                let p = pty.as_mut().unwrap();
                let (action, button, mods) = match event {
                    PointerEvent::Motion { dx, dy, mods } => {
                        mouse.0 = (mouse.0 as i64 + i64::from(dx))
                            .clamp(0, d.width.saturating_sub(1) as i64)
                            as usize;
                        mouse.1 = (mouse.1 as i64 + i64::from(dy))
                            .clamp(0, d.height.saturating_sub(1) as i64)
                            as usize;
                        (2, held_button, mods)
                    }
                    PointerEvent::Button {
                        button,
                        pressed,
                        mods,
                    } => {
                        if pressed {
                            held_button = button;
                        } else if held_button == button {
                            held_button = 0;
                        }
                        (if pressed { 0 } else { 1 }, button, mods)
                    }
                    PointerEvent::Wheel { delta, mods } => {
                        if t.mouse_tracking() && mods & 1 == 0 {
                            for _ in 0..delta.unsigned_abs().min(20) {
                                p.send(&t.mouse(
                                    0,
                                    if delta > 0 { 4 } else { 5 },
                                    mouse.0 as f32,
                                    mouse.1 as f32,
                                    mods,
                                )?)?;
                            }
                        }
                        continue;
                    }
                };
                renderer.pointer = Some(mouse);
                dirty = true;
                let in_terminal = mouse.1 < usize::from(t.rows) * renderer.cell_height();
                if action == 0
                    && button == 2
                    && let Some(selection) = renderer.selection
                {
                    let text = selection.text(&t.frame()?);
                    if p.input_allowed(features) {
                        clipboard.copy(text.clone());
                        p.send(&t.paste(&text))?;
                    }
                    renderer.selection = None;
                    continue;
                }
                if t.mouse_tracking() && mods & 1 == 0 && !selecting {
                    if in_terminal || action == 1 {
                        let y = mouse
                            .1
                            .min(usize::from(t.rows) * renderer.cell_height() - 1);
                        p.send(&t.mouse(action, button, mouse.0 as f32, y as f32, mods)?)?;
                    }
                    continue;
                }
                let cell = (
                    (mouse.0 / renderer.cell_width()).min(usize::from(t.cols) - 1),
                    (mouse.1 / renderer.cell_height()).min(usize::from(t.rows) - 1),
                );
                if action == 0 && button == 1 && in_terminal {
                    selecting = true;
                    renderer.selection = Some(Selection {
                        anchor: cell,
                        head: cell,
                    });
                } else if action == 2 && selecting {
                    if let Some(selection) = renderer.selection.as_mut() {
                        selection.head = cell;
                    }
                } else if action == 1 && button == 1 {
                    selecting = false;
                } else if action == 0 && button == 2 {
                    if let Some(selection) = renderer.selection {
                        let text = selection.text(&t.frame()?);
                        if p.input_allowed(features) {
                            clipboard.copy(text.clone());
                            p.send(&t.paste(&text))?;
                        }
                        renderer.selection = None;
                    }
                } else if action == 0 && button == 3 {
                    clipboard.paste();
                }
            }
        }
        for reply in clipboard.replies.try_iter() {
            if clipboard.is_current(&reply)
                && let (Some(t), Some(p)) = (terminal.as_mut(), pty.as_mut())
                && p.input_allowed(features)
            {
                p.send(&t.paste(&reply.text))?;
            }
        }
        // A switch request can queue a disable immediately. Service it before drawing.
        seat.borrow_mut().dispatch(0)?;
        if !queue.borrow().is_empty() {
            continue;
        }
        if let (Some(d), Some(t), Some(p)) = (display.as_ref(), terminal.as_mut(), pty.as_mut()) {
            let rows = (d.height / renderer.cell_height())
                .saturating_sub(renderer.panel_rows(enabled, overlay.as_ref()))
                .min(300) as u16;
            if rows != t.rows {
                t.resize(t.cols, rows)?;
                p.resize(t.cols, rows, renderer.cell_width(), renderer.cell_height())?;
                renderer.selection = None;
                selecting = false;
                dirty = true;
            }
        }
        if let Some(d) = display.as_mut() {
            let mut poll = libc::pollfd {
                fd: d.fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 0) } > 0
                && let Err(error) = d.events()
            {
                if crate::display::device_transition(&error) {
                    display.take();
                    if let Some(k) = keyboard.as_mut() {
                        k.suspend();
                    }
                    pending_enable = true;
                    next_display_attempt = Instant::now() + Duration::from_millis(30);
                    dirty = true;
                    continue;
                } else {
                    return Err(error.into());
                }
            }
            if dirty && !d.busy() {
                let frame = terminal.as_mut().unwrap().frame()?;
                let pixels = renderer.frame(
                    &frame,
                    d.width,
                    d.height,
                    overlay.as_ref(),
                    enabled && input_safe,
                );
                if let Err(error) = d.present(&pixels) {
                    if crate::display::device_transition(&error) {
                        display.take();
                        if let Some(k) = keyboard.as_mut() {
                            k.suspend();
                        }
                        pending_enable = true;
                        next_display_attempt = Instant::now() + Duration::from_millis(30);
                        dirty = true;
                        continue;
                    } else {
                        return Err(error.into());
                    }
                }
                dirty = false;
            }
        }
        let mut fds = Vec::new();
        fds.push(libc::pollfd {
            fd: seat.borrow_mut().get_fd()?.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        if let Some(k) = keyboard.as_ref().filter(|_| display.is_some()) {
            for fd in k.fds() {
                fds.push(libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
        }
        if let Some(p) = pty.as_ref() {
            fds.push(libc::pollfd {
                fd: p.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        if let Some(d) = display.as_ref() {
            fds.push(libc::pollfd {
                fd: d.fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 16) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
    }
    Ok(())
}
fn vt_key(code: u32) -> Option<i32> {
    match code {
        59..=68 => Some((code - 58) as i32),
        87 => Some(11),
        88 => Some(12),
        _ => None,
    }
}
