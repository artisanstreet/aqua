use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::time::Duration;

use aqua_shell::open::{Destination, ExternalTarget};
use zbus::zvariant::{Fd, OwnedObjectPath, Value};

use crate::{Bus, Method, PortalError};

pub use aqua_shell::open::OpenError;

const CALL_TIMEOUT: Duration = Duration::from_secs(3);

const fn open_uri(member: &'static str) -> Method {
    Method {
        service: "org.freedesktop.portal.Desktop",
        path: "/org/freedesktop/portal/desktop",
        interface: "org.freedesktop.portal.OpenURI",
        member,
    }
}

const OPEN_URI: Method = open_uri("OpenURI");
const OPEN_FILE: Method = open_uri("OpenFile");
const OPEN_DIRECTORY: Method = open_uri("OpenDirectory");

const SHOW_ITEMS: Method = Method {
    service: "org.freedesktop.FileManager1",
    path: "/org/freedesktop/FileManager1",
    interface: "org.freedesktop.FileManager1",
    member: "ShowItems",
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Origin {
    parent: String,
    activation_token: Option<String>,
}

fn clean(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && !text.chars().any(char::is_control)).then(|| text.to_owned())
}

impl Origin {
    pub fn wayland(exported_handle: &str) -> Origin {
        Origin {
            parent: clean(exported_handle)
                .map(|handle| format!("wayland:{handle}"))
                .unwrap_or_default(),
            activation_token: None,
        }
    }

    pub fn with_activation_token(self, token: &str) -> Origin {
        Origin {
            activation_token: clean(token),
            ..self
        }
    }

    pub fn parent(&self) -> &str {
        &self.parent
    }

    fn options(&self) -> HashMap<&'static str, Value<'_>> {
        self.activation_token
            .iter()
            .map(|token| ("activation_token", Value::from(token.as_str())))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTarget(ExternalTarget);

fn desktop_error(error: PortalError) -> OpenError {
    OpenError::Desktop(error.to_string())
}

fn descriptor(path: &Path) -> Result<OwnedFd, OpenError> {
    rustix::fs::open(
        path,
        rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| OpenError::Io(error.into()))
}

impl OpenTarget {
    pub fn url(input: &str) -> Result<Self, OpenError> {
        ExternalTarget::url(input).map(Self)
    }

    pub fn path(path: &Path) -> Result<Self, OpenError> {
        ExternalTarget::path(path).map(Self)
    }

    pub fn reveal(path: &Path) -> Result<Self, OpenError> {
        ExternalTarget::reveal(path).map(Self)
    }

    pub fn execute(&self) -> Result<(), OpenError> {
        let bus = Bus::session().map_err(desktop_error)?;
        self.execute_on(&bus, &Origin::default())
    }

    pub fn execute_on(&self, bus: &Bus, origin: &Origin) -> Result<(), OpenError> {
        let options = origin.options();
        let parent = origin.parent.as_str();
        let opened = match self.0.destination() {
            Destination::Url(url) => bus.call::<_, OwnedObjectPath>(
                OPEN_URI,
                &(parent, url.as_str(), &options),
                CALL_TIMEOUT,
            ),
            Destination::Path(path) => {
                let file = descriptor(path)?;
                bus.call::<_, OwnedObjectPath>(
                    OPEN_FILE,
                    &(parent, Fd::from(&file), &options),
                    CALL_TIMEOUT,
                )
            }
            Destination::Reveal(path) => {
                let file = descriptor(path)?;
                return match bus.call::<_, OwnedObjectPath>(
                    OPEN_DIRECTORY,
                    &(parent, Fd::from(&file), &options),
                    CALL_TIMEOUT,
                ) {
                    Ok(_) => Ok(()),
                    Err(PortalError::Missing { .. }) => show_items(bus, path, origin),
                    Err(error) => Err(desktop_error(error)),
                };
            }
        };
        match opened {
            Ok(_) => Ok(()),
            Err(PortalError::Missing { .. }) => Err(OpenError::Unsupported(
                "the session bus has no org.freedesktop.portal.OpenURI",
            )),
            Err(error) => Err(desktop_error(error)),
        }
    }
}

fn show_items(bus: &Bus, path: &Path, origin: &Origin) -> Result<(), OpenError> {
    let uri = url::Url::from_file_path(path).map_err(|()| OpenError::InvalidPath)?;
    let startup = origin.activation_token.as_deref().unwrap_or_default();
    match bus.call::<_, ()>(SHOW_ITEMS, &(vec![uri.as_str()], startup), CALL_TIMEOUT) {
        Ok(()) => Ok(()),
        Err(PortalError::Missing { .. }) => Err(OpenError::Unsupported(
            "the session bus has neither the OpenURI portal nor org.freedesktop.FileManager1",
        )),
        Err(error) => Err(desktop_error(error)),
    }
}
