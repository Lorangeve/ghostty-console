//! Native notification server on an idle user bus; mirror an existing desktop server.
use crate::buffers::Message;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
};
use zbus::{blocking::MessageIterator, zvariant::OwnedValue};
pub fn session_address() -> String {
    std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_else(|_| {
        let runtime = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::geteuid() }));
        format!("unix:path={runtime}/bus")
    })
}
pub fn spawn() -> Receiver<Message> {
    let (tx, rx) = mpsc::sync_channel(128);
    std::thread::spawn(move || {
        let active = Arc::new(AtomicBool::new(false));
        let address = session_address();
        let server = serve(&address, tx.clone(), active.clone());
        if let Err(error) = &server {
            let _ = tx.try_send(Message::new(
                "通知",
                &format!("独立通知服务不可用: {error}"),
            ));
        }
        if let Err(error) = monitor_inner(&address, &tx, Some(&active)) {
            let _ = tx.try_send(Message::new("通知", &format!("通知同步不可用: {error}")));
        }
        // If monitor access is denied, the native server still accepts Notify directly.
        if let Ok(Some(_server)) = server {
            loop {
                std::thread::park();
            }
        }
    });
    rx
}
pub fn monitor(address: &str, tx: &SyncSender<Message>) -> Result<(), Box<dyn std::error::Error>> {
    monitor_inner(address, tx, None)
}
fn monitor_inner(
    address: &str,
    tx: &SyncSender<Message>,
    active: Option<&AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    struct Reset<'a>(Option<&'a AtomicBool>);
    impl Drop for Reset<'_> {
        fn drop(&mut self) {
            if let Some(flag) = self.0 {
                flag.store(false, Ordering::Release);
            }
        }
    }
    let _reset = Reset(active);
    let connection = zbus::blocking::connection::Builder::address(address)?.build()?;
    let mut messages = MessageIterator::from(&connection);
    // Only notification method calls; never monitor arbitrary desktop traffic.
    let rules = vec![
        "type='method_call',interface='org.freedesktop.Notifications',member='Notify'".to_string(),
    ];
    connection.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus.Monitoring"),
        "BecomeMonitor",
        &(rules, 0u32),
    )?;
    if let Some(active) = active {
        active.store(true, Ordering::Release);
    }
    let _ = tx.try_send(Message::new("通知", "通知接收已连接"));
    for result in &mut messages {
        let message = result?;
        let header = message.header();
        if header.interface().map(|s| s.as_str()) != Some("org.freedesktop.Notifications")
            || header.member().map(|s| s.as_str()) != Some("Notify")
        {
            continue;
        }
        type Notify = (
            String,
            u32,
            String,
            String,
            String,
            Vec<String>,
            HashMap<String, OwnedValue>,
            i32,
        );
        if let Ok((app, _, _, title, body, _, _, _)) = message.body().deserialize::<Notify>() {
            let text = if body.is_empty() {
                title
            } else {
                format!("{title}\n{body}")
            };
            // Bounded queue keeps floods out of the render loop.
            if let Err(mpsc::TrySendError::Disconnected(_)) = tx.try_send(Message::new(&app, &text))
            {
                break;
            }
        }
    }
    if let Some(active) = active {
        active.store(false, Ordering::Release);
    }
    connection.close()?;
    Ok(())
}

struct NotificationServer {
    tx: SyncSender<Message>,
    mirrored: Arc<AtomicBool>,
    next_id: AtomicU32,
}
#[zbus::interface(name = "org.freedesktop.Notifications")]
impl NotificationServer {
    fn get_capabilities(&self) -> Vec<&str> {
        vec!["body"]
    }
    fn get_server_information(&self) -> (&str, &str, &str, &str) {
        (
            "ghostty-console",
            "ghostty-console",
            env!("CARGO_PKG_VERSION"),
            "1.2",
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        if !self.mirrored.load(Ordering::Acquire) {
            let _ = self
                .tx
                .try_send(Message::new(&app_name, &format!("{summary}\n{body}")));
        }
        if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        }
    }
    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        Self::notification_closed(&emitter, id, 3)
            .await
            .map_err(Into::into)
    }
    #[zbus(signal)]
    async fn notification_closed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}
/// Never replace an existing desktop owner. Let a later desktop server replace con.
pub fn serve(
    address: &str,
    tx: SyncSender<Message>,
    mirrored: Arc<AtomicBool>,
) -> Result<Option<zbus::blocking::Connection>, Box<dyn std::error::Error>> {
    let connection = zbus::blocking::connection::Builder::address(address)?
        .serve_at(
            "/org/freedesktop/Notifications",
            NotificationServer {
                tx,
                mirrored,
                next_id: AtomicU32::new(1),
            },
        )?
        .build()?;
    match connection.request_name_with_flags(
        "org.freedesktop.Notifications",
        zbus::fdo::RequestNameFlags::DoNotQueue | zbus::fdo::RequestNameFlags::AllowReplacement,
    ) {
        Ok(_) => Ok(Some(connection)),
        Err(zbus::Error::NameTaken) => Ok(None),
        Err(error) => Err(error.into()),
    }
}
