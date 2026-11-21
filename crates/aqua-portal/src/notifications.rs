pub mod activation;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use zbus::message::Message;
use zbus::zvariant::{OwnedValue, Value};

use crate::bus::{Listener, Signal};
use crate::{Bus, Method, PortalError};

pub use aqua_shell::notifications::{
    native_tag, AppIdentity, Notification, NotificationAction, NotificationError,
    NotificationResponse, ResponseHandler,
};

const CALL_TIMEOUT: Duration = Duration::from_secs(3);
const ACTIVE_LIMIT: usize = 256;
const ACTIVATE: &str = "default";
const ACTION_PREFIX: &str = "action:";

const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_INTERFACE: &str = "org.freedesktop.portal.Notification";
const FREEDESKTOP_PATH: &str = "/org/freedesktop/Notifications";
const FREEDESKTOP_INTERFACE: &str = "org.freedesktop.Notifications";

const fn portal(member: &'static str) -> Method {
    Method {
        service: "org.freedesktop.portal.Desktop",
        path: PORTAL_PATH,
        interface: PORTAL_INTERFACE,
        member,
    }
}

const fn freedesktop(member: &'static str) -> Method {
    Method {
        service: "org.freedesktop.Notifications",
        path: FREEDESKTOP_PATH,
        interface: FREEDESKTOP_INTERFACE,
        member,
    }
}

const ADD: Method = portal("AddNotification");
const REMOVE: Method = portal("RemoveNotification");
const NOTIFY: Method = freedesktop("Notify");
const CLOSE: Method = freedesktop("CloseNotification");
const CAPABILITIES: Method = freedesktop("GetCapabilities");

const SIGNALS: &[Signal] = &[
    Signal {
        path: PORTAL_PATH,
        interface: PORTAL_INTERFACE,
        member: "ActionInvoked",
    },
    Signal {
        path: FREEDESKTOP_PATH,
        interface: FREEDESKTOP_INTERFACE,
        member: "ActionInvoked",
    },
    Signal {
        path: FREEDESKTOP_PATH,
        interface: FREEDESKTOP_INTERFACE,
        member: "NotificationClosed",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Portal,
    Freedesktop,
}

impl Route {
    pub fn for_this_process() -> Route {
        let sandboxed =
            std::path::Path::new("/.flatpak-info").exists() || std::env::var_os("SNAP").is_some();
        match sandboxed {
            true => Route::Portal,
            false => Route::Freedesktop,
        }
    }

    fn other(self) -> Route {
        match self {
            Route::Portal => Route::Freedesktop,
            Route::Freedesktop => Route::Portal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Routing {
    Untried { first: Route },
    Settled(Route),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    Portal(String),
    Freedesktop(u32),
}

type Active = Arc<Mutex<HashMap<String, Shown>>>;

fn lock(active: &Mutex<HashMap<String, Shown>>) -> MutexGuard<'_, HashMap<String, Shown>> {
    active.lock().unwrap_or_else(|error| error.into_inner())
}

fn action_key(id: &str) -> String {
    format!("{ACTION_PREFIX}{id}")
}

fn response(tag: String, key: &str) -> Option<NotificationResponse> {
    match key {
        ACTIVATE => Some(NotificationResponse {
            tag,
            action_id: None,
        }),
        key => key
            .strip_prefix(ACTION_PREFIX)
            .map(|action| NotificationResponse {
                tag,
                action_id: Some(action.to_owned()),
            }),
    }
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn native_error(error: PortalError) -> NotificationError {
    match error {
        PortalError::Missing { .. } => NotificationError::Unsupported,
        error => NotificationError::Native(error.to_string()),
    }
}

fn claim(active: &Active, shown: &Shown) -> Option<String> {
    let mut active = lock(active);
    let tag = active
        .iter()
        .find(|(_, current)| *current == shown)
        .map(|(tag, _)| tag.clone())?;
    active.remove(&tag);
    Some(tag)
}

fn dispatch(active: &Active, respond: &ResponseHandler, message: &Message) {
    let header = message.header();
    let interface = header.interface().map(|name| name.as_str().to_owned());
    let member = header.member().map(|name| name.as_str().to_owned());
    let body = message.body();
    let answer = match (interface.as_deref(), member.as_deref()) {
        (Some(PORTAL_INTERFACE), Some("ActionInvoked")) => body
            .deserialize::<(String, String, Vec<OwnedValue>)>()
            .ok()
            .and_then(|(id, key, _)| {
                claim(active, &Shown::Portal(id)).and_then(|tag| response(tag, &key))
            }),
        (Some(FREEDESKTOP_INTERFACE), Some("ActionInvoked")) => body
            .deserialize::<(u32, String)>()
            .ok()
            .and_then(|(id, key)| {
                claim(active, &Shown::Freedesktop(id)).and_then(|tag| response(tag, &key))
            }),
        (Some(FREEDESKTOP_INTERFACE), Some("NotificationClosed")) => {
            if let Ok((id, _)) = body.deserialize::<(u32, u32)>() {
                claim(active, &Shown::Freedesktop(id));
            }
            None
        }
        _ => None,
    };
    if let Some(answer) = answer {
        respond(answer);
    }
}

pub struct NotificationCenter {
    app: AppIdentity,
    bus: Bus,
    routing: Routing,
    markup: Option<bool>,
    active: Active,
    _listener: Listener,
}

impl NotificationCenter {
    pub fn new(
        identity: AppIdentity,
        response: ResponseHandler,
    ) -> Result<Self, NotificationError> {
        let bus = Bus::session().map_err(native_error)?;
        Self::on(bus, identity, response, Route::for_this_process())
    }

    pub fn on(
        bus: Bus,
        identity: AppIdentity,
        response: ResponseHandler,
        first: Route,
    ) -> Result<Self, NotificationError> {
        let active: Active = Arc::new(Mutex::new(HashMap::new()));
        let shared = active.clone();
        let listener = bus
            .listen("notifications", SIGNALS, move |message| {
                dispatch(&shared, &response, message)
            })
            .map_err(native_error)?;
        Ok(Self {
            app: identity,
            bus,
            routing: Routing::Untried { first },
            markup: None,
            active,
            _listener: listener,
        })
    }

    pub fn route(&self) -> Option<Route> {
        match self.routing {
            Routing::Untried { .. } => None,
            Routing::Settled(route) => Some(route),
        }
    }

    pub fn show(&mut self, notification: Notification) -> Result<(), NotificationError> {
        notification.validate()?;
        let previous = {
            let active = lock(&self.active);
            if active.len() >= ACTIVE_LIMIT && !active.contains_key(&notification.tag) {
                return Err(NotificationError::TooManyActive);
            }
            active.get(&notification.tag).cloned()
        };
        let routes = match self.routing {
            Routing::Untried { first } => vec![first, first.other()],
            Routing::Settled(route) => vec![route],
        };
        let mut failure = None;
        for route in routes {
            let shown = match route {
                Route::Portal => self.show_portal(&notification),
                Route::Freedesktop => self.show_freedesktop(&notification, previous.as_ref()),
            };
            match shown {
                Ok(shown) => {
                    self.routing = Routing::Settled(route);
                    lock(&self.active).insert(notification.tag, shown);
                    return Ok(());
                }
                Err(error) => failure = Some(error),
            }
        }
        Err(native_error(failure.unwrap_or_else(|| {
            panic!("aqua: a notification was shown through no route")
        })))
    }

    fn show_portal(&self, notification: &Notification) -> Result<Shown, PortalError> {
        let id = notification.native_tag();
        let buttons: Vec<HashMap<&str, Value<'_>>> = notification
            .actions
            .iter()
            .map(|action| {
                HashMap::from([
                    ("label", Value::from(action.label.as_str())),
                    ("action", Value::from(action_key(&action.id))),
                ])
            })
            .collect();
        let mut content: HashMap<&str, Value<'_>> = HashMap::from([
            ("title", Value::from(notification.title.as_str())),
            ("body", Value::from(notification.body.as_str())),
            ("default-action", Value::from(ACTIVATE)),
        ]);
        if !buttons.is_empty() {
            content.insert("buttons", Value::from(buttons));
        }
        self.bus
            .call::<_, ()>(ADD, &(id.as_str(), content), CALL_TIMEOUT)?;
        Ok(Shown::Portal(id))
    }

    fn markup(&mut self) -> bool {
        if let Some(markup) = self.markup {
            return markup;
        }
        let markup = self
            .bus
            .call::<_, Vec<String>>(CAPABILITIES, &(), CALL_TIMEOUT)
            .is_ok_and(|capabilities| capabilities.iter().any(|name| name == "body-markup"));
        self.markup = Some(markup);
        markup
    }

    fn show_freedesktop(
        &mut self,
        notification: &Notification,
        previous: Option<&Shown>,
    ) -> Result<Shown, PortalError> {
        let replaces = match previous {
            Some(Shown::Freedesktop(id)) => *id,
            _ => 0,
        };
        let body = match self.markup() {
            true => escape_markup(&notification.body),
            false => notification.body.clone(),
        };
        let actions: Vec<String> = std::iter::once([ACTIVATE.to_owned(), "Open".to_owned()])
            .chain(
                notification
                    .actions
                    .iter()
                    .map(|action| [action_key(&action.id), action.label.clone()]),
            )
            .flatten()
            .collect();
        let hints: HashMap<&str, Value<'_>> =
            HashMap::from([("desktop-entry", Value::from(self.app.identifier()))]);
        let id = self.bus.call::<_, u32>(
            NOTIFY,
            &(
                self.app.name(),
                replaces,
                "",
                notification.title.as_str(),
                body.as_str(),
                actions,
                hints,
                -1i32,
            ),
            CALL_TIMEOUT,
        )?;
        Ok(Shown::Freedesktop(id))
    }

    pub fn withdraw(&mut self, tag: &str) -> Result<(), NotificationError> {
        let shown = lock(&self.active).remove(tag);
        let withdrawn = match (shown, self.routing) {
            (Some(Shown::Portal(id)), _) => {
                self.bus
                    .call::<_, ()>(REMOVE, &(id.as_str(),), CALL_TIMEOUT)
            }
            (None, Routing::Settled(Route::Portal)) => {
                self.bus
                    .call::<_, ()>(REMOVE, &(native_tag(tag).as_str(),), CALL_TIMEOUT)
            }
            (Some(Shown::Freedesktop(id)), _) => {
                self.bus.call::<_, ()>(CLOSE, &(id,), CALL_TIMEOUT)
            }
            (None, _) => Ok(()),
        };
        match withdrawn {
            Ok(()) | Err(PortalError::Missing { .. }) => Ok(()),
            Err(error) => Err(NotificationError::Native(error.to_string())),
        }
    }
}
