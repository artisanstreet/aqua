#![allow(dead_code)]

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

use aqua_portal::testing::PrivateBus;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};

pub const PORTAL: &str = "org.freedesktop.portal.Desktop";
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
pub const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    OpenUri {
        parent: String,
        uri: String,
        token: Option<String>,
    },
    OpenFile {
        parent: String,
        path: PathBuf,
    },
    OpenDirectory {
        parent: String,
        path: PathBuf,
    },
    ShowItems {
        uris: Vec<String>,
        startup: String,
    },
    Added {
        id: String,
        title: String,
        body: String,
        default_action: Option<String>,
        buttons: Vec<(String, String)>,
    },
    Removed {
        id: String,
    },
    Notified {
        id: u32,
        replaces: u32,
        summary: String,
        body: String,
        actions: Vec<String>,
        desktop_entry: Option<String>,
    },
    Closed {
        id: u32,
    },
}

#[derive(Clone, Debug)]
pub enum Setting {
    Unsigned(u32),
    Signed(i32),
    Float(f64),
    Bool(bool),
    Text(&'static str),
}

impl Setting {
    pub fn value(&self) -> OwnedValue {
        let value = match self {
            Setting::Unsigned(value) => Value::from(*value),
            Setting::Signed(value) => Value::from(*value),
            Setting::Float(value) => Value::from(*value),
            Setting::Bool(value) => Value::from(*value),
            Setting::Text(value) => Value::from(*value),
        };
        OwnedValue::try_from(value).expect("settings values have no descriptors")
    }
}

pub type Values = Vec<(&'static str, &'static str, Setting)>;

pub fn bus() -> Option<PrivateBus> {
    PrivateBus::start()
}

fn descriptor_path(fd: &OwnedFd) -> PathBuf {
    std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .expect("the descriptor resolves through /proc")
}

fn text(options: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
}

fn request() -> OwnedObjectPath {
    OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/1_1/aqua")
        .expect("valid request path")
}

pub struct Settings {
    values: Values,
    hang: bool,
}

#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl Settings {
    async fn read_all(
        &self,
        namespaces: Vec<String>,
    ) -> HashMap<String, HashMap<String, OwnedValue>> {
        if self.hang {
            std::future::pending::<()>().await;
        }
        let mut all: HashMap<String, HashMap<String, OwnedValue>> = HashMap::new();
        for (namespace, key, setting) in &self.values {
            if namespaces.iter().any(|wanted| wanted == namespace) {
                all.entry((*namespace).to_owned())
                    .or_default()
                    .insert((*key).to_owned(), setting.value());
            }
        }
        all
    }
}

pub struct OpenUri {
    events: Mutex<Sender<Event>>,
    directories: bool,
}

impl OpenUri {
    fn send(&self, event: Event) {
        let _ = self.events.lock().unwrap().send(event);
    }
}

#[zbus::interface(name = "org.freedesktop.portal.OpenURI")]
impl OpenUri {
    #[zbus(name = "OpenURI")]
    async fn open_uri(
        &self,
        parent: String,
        uri: String,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        self.send(Event::OpenUri {
            parent,
            uri,
            token: text(&options, "activation_token"),
        });
        request()
    }

    async fn open_file(
        &self,
        parent: String,
        fd: OwnedFd,
        _options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        self.send(Event::OpenFile {
            parent,
            path: descriptor_path(&fd),
        });
        request()
    }

    async fn open_directory(
        &self,
        parent: String,
        fd: OwnedFd,
        _options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        if !self.directories {
            return Err(zbus::fdo::Error::UnknownMethod(
                "OpenDirectory needs version 3".into(),
            ));
        }
        self.send(Event::OpenDirectory {
            parent,
            path: descriptor_path(&fd),
        });
        Ok(request())
    }
}

pub struct FileManager {
    events: Mutex<Sender<Event>>,
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager {
    async fn show_items(&self, uris: Vec<String>, startup: String) {
        let _ = self
            .events
            .lock()
            .unwrap()
            .send(Event::ShowItems { uris, startup });
    }
}

pub struct PortalNotifications {
    events: Mutex<Sender<Event>>,
}

fn buttons(content: &HashMap<String, OwnedValue>) -> Vec<(String, String)> {
    content
        .get("buttons")
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<HashMap<String, OwnedValue>>::try_from(value).ok())
        .unwrap_or_default()
        .iter()
        .map(|button| {
            (
                text(button, "label").unwrap_or_default(),
                text(button, "action").unwrap_or_default(),
            )
        })
        .collect()
}

#[zbus::interface(name = "org.freedesktop.portal.Notification")]
impl PortalNotifications {
    async fn add_notification(&self, id: String, content: HashMap<String, OwnedValue>) {
        let _ = self.events.lock().unwrap().send(Event::Added {
            id,
            title: text(&content, "title").unwrap_or_default(),
            body: text(&content, "body").unwrap_or_default(),
            default_action: text(&content, "default-action"),
            buttons: buttons(&content),
        });
    }

    async fn remove_notification(&self, id: String) {
        let _ = self.events.lock().unwrap().send(Event::Removed { id });
    }
}

pub struct Freedesktop {
    events: Mutex<Sender<Event>>,
    next: AtomicU32,
    markup: bool,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Freedesktop {
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        _app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let id = match replaces_id {
            0 => self.next.fetch_add(1, Ordering::Relaxed),
            id => id,
        };
        let _ = self.events.lock().unwrap().send(Event::Notified {
            id,
            replaces: replaces_id,
            summary,
            body,
            actions,
            desktop_entry: text(&hints, "desktop-entry"),
        });
        id
    }

    async fn close_notification(&self, id: u32, #[zbus(connection)] connection: &zbus::Connection) {
        let _ = self.events.lock().unwrap().send(Event::Closed { id });
        let _ = connection
            .emit_signal(
                None::<()>,
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
                "NotificationClosed",
                &(id, 3u32),
            )
            .await;
    }

    async fn get_capabilities(&self) -> Vec<String> {
        let mut capabilities = vec!["actions".to_owned()];
        if self.markup {
            capabilities.push("body-markup".to_owned());
        }
        capabilities
    }
}

#[derive(Default)]
pub struct Desktop {
    pub settings: Option<Values>,
    pub hung_settings: bool,
    pub open_uri: Option<bool>,
    pub notifications: bool,
}

pub struct Service {
    connection: zbus::Connection,
}

impl Service {
    pub fn emit<B>(&self, path: &str, interface: &str, member: &str, body: &B)
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        zbus::block_on(
            self.connection
                .emit_signal(None::<()>, path, interface, member, body),
        )
        .expect("the fake service emits its signal");
    }
}

fn serve(
    bus: &PrivateBus,
    name: &str,
    build: impl FnOnce(
        zbus::connection::Builder<'static>,
    ) -> zbus::Result<zbus::connection::Builder<'static>>,
) -> Service {
    let builder = zbus::connection::Builder::address(bus.address())
        .and_then(|builder| builder.name(name.to_owned()))
        .and_then(build)
        .expect("the fake service is configurable");
    let connection = zbus::block_on(builder.build()).expect("the fake service connects");
    Service { connection }
}

pub fn desktop(bus: &PrivateBus, parts: Desktop) -> (Service, Receiver<Event>) {
    let (events, received) = channel();
    let service = serve(bus, PORTAL, |mut builder| {
        if let Some(values) = parts.settings {
            builder = builder.serve_at(
                PORTAL_PATH,
                Settings {
                    values,
                    hang: parts.hung_settings,
                },
            )?;
        }
        if let Some(directories) = parts.open_uri {
            builder = builder.serve_at(
                PORTAL_PATH,
                OpenUri {
                    events: Mutex::new(events.clone()),
                    directories,
                },
            )?;
        }
        if parts.notifications {
            builder = builder.serve_at(
                PORTAL_PATH,
                PortalNotifications {
                    events: Mutex::new(events.clone()),
                },
            )?;
        }
        Ok(builder)
    });
    (service, received)
}

pub fn file_manager(bus: &PrivateBus) -> (Service, Receiver<Event>) {
    let (events, received) = channel();
    let service = serve(bus, "org.freedesktop.FileManager1", |builder| {
        builder.serve_at(
            "/org/freedesktop/FileManager1",
            FileManager {
                events: Mutex::new(events),
            },
        )
    });
    (service, received)
}

pub fn freedesktop_notifications(bus: &PrivateBus, markup: bool) -> (Service, Receiver<Event>) {
    let (events, received) = channel();
    let service = serve(bus, "org.freedesktop.Notifications", |builder| {
        builder.serve_at(
            "/org/freedesktop/Notifications",
            Freedesktop {
                events: Mutex::new(events),
                next: AtomicU32::new(7),
                markup,
            },
        )
    });
    (service, received)
}

pub fn next(events: &Receiver<Event>) -> Event {
    events
        .recv_timeout(WAIT)
        .expect("the fake service records the call")
}
