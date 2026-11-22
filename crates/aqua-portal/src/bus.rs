use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use futures_lite::{future, Stream, StreamExt};
use zbus::message::{Message, Type as MessageType};
use zbus::zvariant::DynamicType;
use zbus::{Connection, MatchRule, MessageStream};

pub type Wake = Arc<dyn Fn() + Send + Sync>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Method {
    pub service: &'static str,
    pub path: &'static str,
    pub interface: &'static str,
    pub member: &'static str,
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{} on {}", self.interface, self.member, self.service)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PortalError {
    #[error("cannot connect to the D-Bus {bus}: {reason}")]
    Connect { bus: String, reason: String },
    #[error("{method} is not available: {reason}")]
    Missing { method: Method, reason: String },
    #[error("{method} failed: {reason}")]
    Failed { method: Method, reason: String },
    #[error("{method} did not answer within {after:?}")]
    Timeout { method: Method, after: Duration },
    #[error("{method} answered with an unexpected body: {reason}")]
    Reply { method: Method, reason: String },
    #[error("cannot subscribe to {interface}.{member}: {reason}")]
    Subscribe {
        interface: &'static str,
        member: &'static str,
        reason: String,
    },
    #[error("cannot emit {interface}.{member} at {path}: {reason}")]
    Emit {
        path: String,
        interface: &'static str,
        member: &'static str,
        reason: String,
    },
}

const MISSING_ERRORS: &[&str] = &[
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.NameHasNoOwner",
    "org.freedesktop.DBus.Error.UnknownMethod",
    "org.freedesktop.DBus.Error.UnknownInterface",
    "org.freedesktop.DBus.Error.UnknownObject",
];

fn call_error(method: Method, error: zbus::Error) -> PortalError {
    let name = match &error {
        zbus::Error::MethodError(name, _, _) => Some(name.as_str().to_owned()),
        zbus::Error::FDO(error) => Some(zbus::DBusError::name(error.as_ref()).as_str().to_owned()),
        _ => None,
    };
    let reason = error.to_string();
    match name {
        Some(name) if MISSING_ERRORS.contains(&name.as_str()) => {
            PortalError::Missing { method, reason }
        }
        _ => PortalError::Failed { method, reason },
    }
}

pub(crate) struct Signal {
    pub path: &'static str,
    pub interface: &'static str,
    pub member: &'static str,
}

#[derive(Clone, Debug)]
pub struct Bus {
    connection: Connection,
}

static SESSION: Mutex<Option<Bus>> = Mutex::new(None);

fn connect_error(bus: &str) -> impl Fn(zbus::Error) -> PortalError + '_ {
    move |error| PortalError::Connect {
        bus: bus.to_owned(),
        reason: error.to_string(),
    }
}

async fn bounded<T>(
    work: impl Future<Output = Result<T, PortalError>>,
    after: Duration,
    expired: impl FnOnce() -> PortalError,
) -> Result<T, PortalError> {
    future::or(work, async {
        async_io::Timer::after(after).await;
        Err(expired())
    })
    .await
}

impl Bus {
    pub fn session() -> Result<Bus, PortalError> {
        let mut shared = SESSION.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(bus) = shared.as_ref() {
            return Ok(bus.clone());
        }
        let bus = Self::build("session bus", zbus::connection::Builder::session)?;
        *shared = Some(bus.clone());
        Ok(bus)
    }

    pub fn connect(address: &str) -> Result<Bus, PortalError> {
        let label = format!("bus at {address}");
        Self::build(&label, || zbus::connection::Builder::address(address))
    }

    fn build<'a>(
        label: &str,
        builder: impl FnOnce() -> zbus::Result<zbus::connection::Builder<'a>>,
    ) -> Result<Bus, PortalError> {
        let builder = builder().map_err(connect_error(label))?;
        let connection = zbus::block_on(bounded(
            async { builder.build().await.map_err(connect_error(label)) },
            CONNECT_TIMEOUT,
            || PortalError::Connect {
                bus: label.to_owned(),
                reason: format!("no answer within {CONNECT_TIMEOUT:?}"),
            },
        ))?;
        Ok(Bus { connection })
    }

    pub fn unique_name(&self) -> Option<String> {
        self.connection
            .unique_name()
            .map(|name| name.as_str().to_owned())
    }

    pub(crate) fn call<B, R>(
        &self,
        method: Method,
        body: &B,
        timeout: Duration,
    ) -> Result<R, PortalError>
    where
        B: serde::Serialize + DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        zbus::block_on(bounded(
            async {
                let reply = self
                    .connection
                    .call_method(
                        Some(method.service),
                        method.path,
                        Some(method.interface),
                        method.member,
                        body,
                    )
                    .await
                    .map_err(|error| call_error(method, error))?;
                reply
                    .body()
                    .deserialize::<R>()
                    .map_err(|error| PortalError::Reply {
                        method,
                        reason: error.to_string(),
                    })
            },
            timeout,
            || PortalError::Timeout {
                method,
                after: timeout,
            },
        ))
    }

    pub(crate) fn emit<B>(
        &self,
        path: &str,
        interface: &'static str,
        member: &'static str,
        body: &B,
    ) -> Result<(), PortalError>
    where
        B: serde::Serialize + DynamicType,
    {
        zbus::block_on(
            self.connection
                .emit_signal(None::<()>, path, interface, member, body),
        )
        .map_err(|error| PortalError::Emit {
            path: path.to_owned(),
            interface,
            member,
            reason: error.to_string(),
        })
    }

    pub(crate) fn listen(
        &self,
        name: &str,
        signals: &[Signal],
        mut handle: impl FnMut(&Message) + Send + 'static,
    ) -> Result<Listener, PortalError> {
        let mut merged: Option<Pin<Box<dyn Stream<Item = zbus::Result<Message>> + Send>>> = None;
        for signal in signals {
            let subscribe_error = |error: zbus::Error| PortalError::Subscribe {
                interface: signal.interface,
                member: signal.member,
                reason: error.to_string(),
            };
            let rule = MatchRule::builder()
                .msg_type(MessageType::Signal)
                .path(signal.path)
                .and_then(|rule| rule.interface(signal.interface))
                .and_then(|rule| rule.member(signal.member))
                .map_err(subscribe_error)?
                .build();
            let stream =
                zbus::block_on(MessageStream::for_match_rule(rule, &self.connection, None))
                    .map_err(subscribe_error)?;
            merged = Some(match merged {
                None => Box::pin(stream),
                Some(previous) => Box::pin(previous.or(stream)),
            });
        }
        let mut stream = merged.unwrap_or_else(|| Box::pin(futures_lite::stream::pending()));
        let (stop, stopped) = async_channel::bounded::<()>(1);
        let thread = std::thread::Builder::new()
            .name(format!("aqua-portal-{name}"))
            .spawn(move || {
                zbus::block_on(future::or(
                    async move {
                        while let Some(message) = stream.next().await {
                            if let Ok(message) = message {
                                handle(&message);
                            }
                        }
                    },
                    async move {
                        let _ = stopped.recv().await;
                    },
                ))
            })
            .unwrap_or_else(|error| panic!("aqua: cannot start the {name} listener: {error}"));
        Ok(Listener {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

pub(crate) struct Listener {
    stop: Option<async_channel::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
    }
}
