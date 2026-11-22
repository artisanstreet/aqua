mod desktop;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use aqua_core::ClickSettings;
use aqua_protocol::prefs::SystemPrefs;
use zbus::zvariant::{OwnedValue, Value};

use crate::bus::{Listener, Signal};
use crate::{Bus, Method, PortalError, Wake};
use desktop::{Desktop, Key, Reading};

pub use aqua_protocol::prefs::{prefers_opaque_frame, should_reduce_motion};

pub const STARTUP_TIMEOUT: Duration = Duration::from_millis(300);

const READ_ALL: Method = Method {
    service: "org.freedesktop.portal.Desktop",
    path: "/org/freedesktop/portal/desktop",
    interface: "org.freedesktop.portal.Settings",
    member: "ReadAll",
};

const SETTING_CHANGED: Signal = Signal {
    path: "/org/freedesktop/portal/desktop",
    interface: "org.freedesktop.portal.Settings",
    member: "SettingChanged",
};

enum Phase {
    Reading { desktop: Desktop, changed: Vec<Key> },
    Portal(Desktop),
    Fallback,
}

impl Phase {
    fn view(&self) -> (SystemPrefs, ClickSettings) {
        match self {
            Phase::Reading { desktop, .. } | Phase::Portal(desktop) => {
                (desktop.system_prefs(), desktop.click_settings())
            }
            Phase::Fallback => (SystemPrefs::fallback(), desktop::default_click_settings()),
        }
    }

    fn change(&mut self, key: Key, reading: &Reading) {
        match self {
            Phase::Reading { desktop, changed } => {
                desktop.set(key, reading);
                changed.push(key);
            }
            Phase::Portal(desktop) => desktop.set(key, reading),
            Phase::Fallback => {
                let mut desktop = Desktop::default();
                desktop.set(key, reading);
                *self = Phase::Portal(desktop);
            }
        }
    }

    fn settle(&mut self, read: Result<Vec<(Key, Reading)>, PortalError>) -> Option<PortalError> {
        let Phase::Reading {
            mut desktop,
            changed,
        } = std::mem::replace(self, Phase::Fallback)
        else {
            panic!("aqua: portal settings settled twice");
        };
        match read {
            Ok(readings) => {
                for (key, reading) in readings.iter().filter(|(key, _)| !changed.contains(key)) {
                    desktop.set(*key, reading);
                }
                *self = Phase::Portal(desktop);
                None
            }
            Err(error) => {
                if !changed.is_empty() {
                    *self = Phase::Portal(desktop);
                }
                Some(error)
            }
        }
    }
}

pub struct Preferences {
    phase: Arc<Mutex<Phase>>,
    _listener: Option<Listener>,
}

fn lock(phase: &Mutex<Phase>) -> MutexGuard<'_, Phase> {
    phase.lock().unwrap_or_else(|error| error.into_inner())
}

fn reading(value: &Value<'_>) -> Option<Reading> {
    match value {
        Value::Value(inner) => reading(inner),
        Value::U8(value) => Some(Reading::Unsigned(u64::from(*value))),
        Value::U16(value) => Some(Reading::Unsigned(u64::from(*value))),
        Value::U32(value) => Some(Reading::Unsigned(u64::from(*value))),
        Value::U64(value) => Some(Reading::Unsigned(*value)),
        Value::I16(value) => Some(Reading::Signed(i64::from(*value))),
        Value::I32(value) => Some(Reading::Signed(i64::from(*value))),
        Value::I64(value) => Some(Reading::Signed(*value)),
        Value::F64(value) => Some(Reading::Float(*value)),
        Value::Bool(value) => Some(Reading::Bool(*value)),
        Value::Str(text) => Some(Reading::Text(text.as_str().to_owned())),
        _ => None,
    }
}

fn readings(all: HashMap<String, HashMap<String, OwnedValue>>) -> Vec<(Key, Reading)> {
    all.iter()
        .flat_map(|(namespace, values)| {
            values
                .iter()
                .filter_map(move |(name, value)| Key::parse(namespace, name).zip(reading(value)))
        })
        .collect()
}

impl Preferences {
    pub fn start(wake: Wake) -> Preferences {
        match Bus::session() {
            Ok(bus) => Self::start_on(&bus, wake),
            Err(error) => {
                eprintln!("aqua: system preferences fall back to defaults: {error}");
                Preferences {
                    phase: Arc::new(Mutex::new(Phase::Fallback)),
                    _listener: None,
                }
            }
        }
    }

    pub fn start_on(bus: &Bus, wake: Wake) -> Preferences {
        let phase = Arc::new(Mutex::new(Phase::Reading {
            desktop: Desktop::default(),
            changed: Vec::new(),
        }));
        let shared = phase.clone();
        let listener = bus
            .listen("settings", &[SETTING_CHANGED], move |message| {
                let Ok((namespace, name, value)) =
                    message.body().deserialize::<(String, String, OwnedValue)>()
                else {
                    return;
                };
                let Some((key, reading)) = Key::parse(&namespace, &name).zip(reading(&value))
                else {
                    return;
                };
                let changed = {
                    let mut phase = lock(&shared);
                    let before = phase.view();
                    phase.change(key, &reading);
                    phase.view() != before
                };
                if changed {
                    wake();
                }
            })
            .map_err(|error| eprintln!("aqua: system preference changes are not followed: {error}"))
            .ok();
        let read = bus
            .call::<_, HashMap<String, HashMap<String, OwnedValue>>>(
                READ_ALL,
                &(desktop::NAMESPACES,),
                STARTUP_TIMEOUT,
            )
            .map(readings);
        if let Some(error) = lock(&phase).settle(read) {
            eprintln!("aqua: system preferences fall back to defaults: {error}");
        }
        Preferences {
            phase,
            _listener: listener,
        }
    }

    pub fn query(&self) -> SystemPrefs {
        desktop::with_environment(self.system(), |name| std::env::var(name).ok())
    }

    pub fn system(&self) -> SystemPrefs {
        lock(&self.phase).view().0
    }

    pub fn click_settings(&self) -> ClickSettings {
        lock(&self.phase).view().1
    }
}
