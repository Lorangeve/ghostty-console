//! A private bus proves that monitoring leaves the original notification server in charge.
use ghostty_console::{buffers::Message, notifications};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};
use zbus::zvariant::OwnedValue;
struct Server(mpsc::Sender<String>);
#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app: String,
        _id: u32,
        _icon: String,
        title: String,
        _body: String,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _timeout: i32,
    ) -> u32 {
        self.0.send(title).unwrap();
        42
    }
}
struct Bus(std::process::Child);
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn notification_is_mirrored_and_original_server_still_replies() {
    let mut bus = Bus(Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap());
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim().to_string();
    let (server_tx, server_rx) = mpsc::channel();
    let _server = zbus::blocking::connection::Builder::address(address.as_str())
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at("/org/freedesktop/Notifications", Server(server_tx))
        .unwrap()
        .build()
        .unwrap();
    let (tx, rx) = mpsc::sync_channel::<Message>(8);
    let monitor_address = address.clone();
    let monitor = std::thread::spawn(move || {
        notifications::monitor(&monitor_address, &tx).map_err(|e| e.to_string())
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap()
            .text
            .contains("已连接")
    );
    let client = zbus::blocking::connection::Builder::address(address.as_str())
        .unwrap()
        .build()
        .unwrap();
    let send = || {
        client
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(
                    "test",
                    0u32,
                    "",
                    "测试标题",
                    "中文正文",
                    Vec::<String>::new(),
                    HashMap::<String, OwnedValue>::new(),
                    -1i32,
                ),
            )
            .unwrap()
    };
    let reply = send();
    assert_eq!(reply.body().deserialize::<u32>().unwrap(), 42);
    let message = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(message.source, "test");
    assert_eq!(message.text, "测试标题\n中文正文");
    assert_eq!(
        server_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        "测试标题"
    );
    drop(rx);
    send();
    monitor.join().unwrap().unwrap();
}

#[test]
fn native_server_receives_notify_send_without_a_desktop() {
    use std::sync::{Arc, atomic::AtomicBool};
    let mut bus = Bus(Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap());
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim();
    let (tx, rx) = mpsc::sync_channel(8);
    let _server = notifications::serve(address, tx.clone(), Arc::new(AtomicBool::new(false)))
        .unwrap()
        .unwrap();
    assert!(
        notifications::serve(address, tx, Arc::new(AtomicBool::new(false)))
            .unwrap()
            .is_none()
    );
    let status = Command::new("notify-send")
        .args(["--app-name=con-test", "原生通知", "没有桌面也能接收"])
        .env("DBUS_SESSION_BUS_ADDRESS", address)
        .status()
        .unwrap();
    assert!(status.success());
    let message = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(message.source, "con-test");
    assert_eq!(message.text, "原生通知\n没有桌面也能接收");
}
