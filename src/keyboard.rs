use crate::display::SharedSeat;
use evdev::{Device, EventType, KeyCode, RelativeAxisCode};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    os::fd::{AsFd, AsRawFd},
    path::PathBuf,
    time::{Duration, Instant},
};
use xkbcommon::xkb;
#[derive(Clone)]
pub struct Key {
    pub code: u32,
    pub keysym: u32,
    pub text: String,
    pub unshifted: u32,
    pub mods: u32,
    pub rime_mods: i32,
    pub pressed: bool,
    pub repeated: bool,
}
#[derive(Debug)]
pub enum PointerEvent {
    Motion {
        dx: i32,
        dy: i32,
        mods: u32,
    },
    Button {
        button: i32,
        pressed: bool,
        mods: u32,
    },
    Wheel {
        delta: i32,
        mods: u32,
    },
}
pub struct InputBatch {
    pub keys: Vec<Key>,
    pub pointers: Vec<PointerEvent>,
}
enum Raw {
    Key(u32, i32, bool),
    Motion(i32, i32),
    Button(i32, bool),
    Wheel(i32),
}
struct DeviceEntry {
    input: Option<Device>,
    device: Option<crate::seat::Device>,
    seat: SharedSeat,
    path: PathBuf,
    pressed: HashSet<u32>,
}
impl Drop for DeviceEntry {
    fn drop(&mut self) {
        drop(self.input.take());
        if let Some(d) = self.device.take() {
            let _ = self.seat.borrow_mut().close_device(d);
        }
    }
}
pub struct Keyboard {
    seat: SharedSeat,
    devices: Vec<DeviceEntry>,
    seen: HashSet<PathBuf>,
    state: xkb::State,
    keymap: xkb::Keymap,
    counts: HashMap<u32, usize>,
    next_scan: Instant,
}
impl Keyboard {
    pub fn new(seat: SharedSeat) -> Result<Self, String> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            &crate::settings::get()?.keyboard_layout,
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or("Cannot create XKB keymap")?;
        let state = xkb::State::new(&keymap);
        let mut result = Self {
            seat,
            devices: Vec::new(),
            seen: HashSet::new(),
            state,
            keymap,
            counts: HashMap::new(),
            next_scan: Instant::now(),
        };
        result.scan().map_err(|e| e.to_string())?;
        Ok(result)
    }
    fn scan(&mut self) -> io::Result<()> {
        let paths: HashSet<PathBuf> = fs::read_dir("/dev/input")?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("event")
            })
            .collect();
        self.seen.retain(|p| paths.contains(p));
        for path in paths {
            if !self.seen.insert(path.clone()) {
                continue;
            }
            let opened = self.seat.borrow_mut().open_device(&path);
            let device = match opened {
                Ok(device) => device,
                Err(error) => {
                    self.seen.remove(&path);
                    if crate::display::device_transition(&error) {
                        break;
                    }
                    continue;
                }
            };
            let mut entry = DeviceEntry {
                input: None,
                device: Some(device),
                seat: self.seat.clone(),
                path,
                pressed: HashSet::new(),
            };
            let fd = entry
                .device
                .as_ref()
                .unwrap()
                .as_fd()
                .try_clone_to_owned()?;
            let input = match Device::from_fd(fd) {
                Ok(input) => input,
                Err(error) if crate::display::device_transition(&error) => {
                    self.seen.remove(&entry.path);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let keyboard = input.supported_keys().is_some_and(|keys| {
                keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_ENTER)
            });
            let pointer = input.supported_relative_axes().is_some_and(|axes| {
                axes.contains(RelativeAxisCode::REL_X) || axes.contains(RelativeAxisCode::REL_Y)
            }) || input
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT));
            if !keyboard && !pointer {
                continue;
            }
            input.set_nonblocking(true)?;
            eprintln!(
                "input: {} ({})",
                input.name().unwrap_or("unknown"),
                entry.path.display()
            );
            entry.input = Some(input);
            self.devices.push(entry);
        }
        self.next_scan = Instant::now()
            + if self.devices.is_empty() {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(1)
            };
        Ok(())
    }
    pub fn fds(&self) -> Vec<i32> {
        self.devices
            .iter()
            .map(|d| d.input.as_ref().unwrap().as_raw_fd())
            .collect()
    }
    pub fn suspend(&mut self) {
        self.devices.clear();
        self.seen.clear();
        self.counts.clear();
        self.state = xkb::State::new(&self.keymap);
    }
    pub fn resume(&mut self) -> Result<(), String> {
        self.scan().map_err(|e| e.to_string())
    }
    fn modifiers(&self) -> u32 {
        let mut mods = 0;
        for (name, bit) in [
            (xkb::MOD_NAME_SHIFT, 1),
            (xkb::MOD_NAME_CTRL, 2),
            (xkb::MOD_NAME_ALT, 4),
            (xkb::MOD_NAME_LOGO, 8),
            (xkb::MOD_NAME_CAPS, 16),
            (xkb::MOD_NAME_NUM, 32),
        ] {
            if self
                .state
                .mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE)
            {
                mods |= bit;
            }
        }
        mods
    }
    pub fn events(&mut self) -> io::Result<InputBatch> {
        if Instant::now() >= self.next_scan {
            self.scan()?;
        }
        let mut raw = Vec::new();
        let mut removed = Vec::new();
        for (index, entry) in self.devices.iter_mut().enumerate() {
            let events = match entry.input.as_mut().unwrap().fetch_events() {
                Ok(events) => events.collect::<Vec<_>>(),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) if crate::display::device_transition(&e) => {
                    removed.push(index);
                    continue;
                }
                Err(e) => return Err(e),
            };
            for event in events {
                if event.event_type() == EventType::RELATIVE {
                    match event.code() {
                        0 => raw.push(Raw::Motion(event.value(), 0)),
                        1 => raw.push(Raw::Motion(0, event.value())),
                        8 => raw.push(Raw::Wheel(event.value())),
                        _ => {}
                    }
                    continue;
                }
                if event.event_type() != EventType::KEY || !(0..=2).contains(&event.value()) {
                    continue;
                }
                let code = u32::from(event.code());
                let value = event.value();
                if (272..=274).contains(&code) {
                    let button = match code {
                        272 => 1,
                        273 => 2,
                        _ => 3,
                    };
                    if value != 2 {
                        raw.push(Raw::Button(button, value == 1));
                    }
                    continue;
                }
                let change = if value == 1 {
                    entry.pressed.insert(code)
                } else if value == 0 {
                    entry.pressed.remove(&code)
                } else {
                    false
                };
                raw.push(Raw::Key(code, value, change));
            }
        }
        for index in removed.into_iter().rev() {
            let entry = self.devices.remove(index);
            for code in &entry.pressed {
                raw.push(Raw::Key(*code, 0, true));
            }
            self.seen.remove(&entry.path);
        }
        let mut keys = Vec::new();
        let mut pointers = Vec::new();
        for event in raw {
            let (code, value, change) = match event {
                Raw::Key(code, value, change) => (code, value, change),
                Raw::Motion(dx, dy) => {
                    pointers.push(PointerEvent::Motion {
                        dx,
                        dy,
                        mods: self.modifiers(),
                    });
                    continue;
                }
                Raw::Button(button, pressed) => {
                    pointers.push(PointerEvent::Button {
                        button,
                        pressed,
                        mods: self.modifiers(),
                    });
                    continue;
                }
                Raw::Wheel(delta) => {
                    pointers.push(PointerEvent::Wheel {
                        delta,
                        mods: self.modifiers(),
                    });
                    continue;
                }
            };
            let pressed = value != 0;
            let keycode = xkb::Keycode::new(code + 8);
            if change && pressed {
                let count = self.counts.entry(code).or_default();
                *count += 1;
                if *count == 1 {
                    self.state.update_key(keycode, xkb::KeyDirection::Down);
                }
            }
            let sym = self.state.key_get_one_sym(keycode);
            let mods = self.modifiers();
            let rime_mods = ((mods & 1)
                | ((mods & 2) << 1)
                | ((mods & 4) << 1)
                | ((mods & 16) >> 3)
                | ((mods & 8) << 23)) as i32
                | if pressed { 0 } else { 1 << 30 };
            let unshifted = self
                .keymap
                .key_get_syms_by_level(keycode, 0, 0)
                .first()
                .map(|s| xkb::keysym_to_utf32(*s))
                .unwrap_or(0);
            keys.push(Key {
                code,
                pressed,
                repeated: value == 2,
                keysym: sym.raw(),
                text: xkb::keysym_to_utf8(sym),
                mods,
                rime_mods,
                unshifted,
            });
            if change && !pressed {
                let count = self.counts.entry(code).or_default();
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.state.update_key(keycode, xkb::KeyDirection::Up);
                }
            }
        }
        Ok(InputBatch { keys, pointers })
    }
}

#[cfg(test)]
mod seat_transition_tests {
    use super::*;
    #[test]
    fn an_inactive_broker_during_initial_scan_does_not_end_the_console() {
        use std::os::unix::net::UnixDatagram;
        let has_events = std::fs::read_dir("/dev/input").is_ok_and(|mut d| {
            d.any(|p| p.is_ok_and(|p| p.file_name().to_string_lossy().starts_with("event")))
        });
        if !has_events {
            return;
        }
        let (client, server) = UnixDatagram::pair().unwrap();
        let (events, _) = UnixDatagram::pair().unwrap();
        let mock = std::thread::spawn(move || {
            for _ in 0..2 {
                let (request, fd) = crate::broker::receive_fd(&server).unwrap();
                assert!(fd.is_none());
                assert_eq!(request[0], b'O');
                let mut reply = libc::EAGAIN.to_le_bytes().to_vec();
                reply.extend_from_slice(&0u32.to_le_bytes());
                crate::broker::send_fd(&server, &reply, None).unwrap();
            }
        });
        let seat = std::rc::Rc::new(std::cell::RefCell::new(
            crate::seat::Seat::native(client, events, |_| {}).unwrap(),
        ));
        let mut keyboard = Keyboard::new(seat).unwrap();
        assert!(keyboard.devices.is_empty());
        keyboard.resume().unwrap();
        assert!(keyboard.devices.is_empty());
        mock.join().unwrap();
    }
}
