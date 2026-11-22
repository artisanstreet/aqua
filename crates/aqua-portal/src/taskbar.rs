use std::collections::HashMap;

use aqua_shell::AppId;
use zbus::zvariant::Value;

use crate::Bus;

pub use aqua_protocol::{BadgeError, TaskbarBadge};

const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
const PATH_PREFIX: &str = "/com/canonical/unity/launcherentry/";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Unidentified,
    Identified { uri: String, path: String },
}

#[derive(Debug)]
pub struct Launcher {
    bus: Bus,
    entry: Entry,
}

impl Launcher {
    pub fn new(app_id: Option<AppId>) -> Result<Launcher, BadgeError> {
        let bus = Bus::session().map_err(|error| BadgeError::Native(error.to_string()))?;
        Ok(Self::on(bus, app_id))
    }

    pub fn on(bus: Bus, app_id: Option<AppId>) -> Launcher {
        let entry = match app_id {
            None => Entry::Unidentified,
            Some(app_id) => {
                let uri = format!("application://{app_id}.desktop");
                let path = format!(
                    "{PATH_PREFIX}{}",
                    aqua_shell::notifications::native_tag(&uri)
                );
                Entry::Identified { uri, path }
            }
        };
        Launcher { bus, entry }
    }

    pub fn set_badge(&self, badge: Option<TaskbarBadge>) -> Result<(), BadgeError> {
        let Entry::Identified { uri, path } = &self.entry else {
            return Err(BadgeError::Unsupported);
        };
        let count = badge.map_or(0, |badge| badge.count);
        let properties: HashMap<&str, Value<'_>> = HashMap::from([
            ("count", Value::from(i64::from(count))),
            ("count-visible", Value::from(count > 0)),
        ]);
        self.bus
            .emit(path, INTERFACE, "Update", &(uri.as_str(), properties))
            .map_err(|error| BadgeError::Native(error.to_string()))
    }
}
