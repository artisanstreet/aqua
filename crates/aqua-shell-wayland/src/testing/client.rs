use super::HarnessError;
use std::path::Path;
use std::time::Duration;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

pub(crate) struct Client<S: 'static> {
    pub conn: Connection,
    pub queue: EventQueue<S>,
    pub globals: GlobalList,
    pub state: S,
    pub name: &'static str,
}

pub(crate) fn failed(client: &'static str, reason: impl std::fmt::Display) -> HarnessError {
    HarnessError::Client {
        client,
        reason: reason.to_string(),
    }
}

impl<S: Dispatch<WlRegistry, GlobalListContents> + 'static> Client<S> {
    pub(crate) fn connect(
        display: &Path,
        name: &'static str,
        state: S,
    ) -> Result<Self, HarnessError> {
        let stream = std::os::unix::net::UnixStream::connect(display)
            .map_err(|error| failed(name, error))?;
        let conn = Connection::from_socket(stream).map_err(|error| failed(name, error))?;
        let (globals, queue) =
            registry_queue_init::<S>(&conn).map_err(|error| failed(name, error))?;
        Ok(Self {
            conn,
            queue,
            globals,
            state,
            name,
        })
    }

    pub(crate) fn handle(&self) -> QueueHandle<S> {
        self.queue.handle()
    }

    pub(crate) fn bind<I, U>(&self, version: u32, data: U) -> Result<I, HarnessError>
    where
        I: Proxy + 'static,
        U: Send + Sync + 'static,
        S: Dispatch<I, U>,
    {
        self.globals
            .bind::<I, S, U>(&self.handle(), 1..=version, data)
            .map_err(|error| failed(self.name, format!("{}: {error}", I::interface().name)))
    }

    pub(crate) fn flush(&self) -> Result<(), HarnessError> {
        self.conn.flush().map_err(|error| failed(self.name, error))
    }

    pub(crate) fn roundtrip(&mut self) -> Result<(), HarnessError> {
        self.queue
            .roundtrip(&mut self.state)
            .map(|_| ())
            .map_err(|error| failed(self.name, error))
    }

    pub(crate) fn dispatch_until(
        &mut self,
        limit: Duration,
        done: impl Fn(&S) -> bool,
    ) -> Result<(), HarnessError> {
        let started = std::time::Instant::now();
        while !done(&self.state) {
            if started.elapsed() > limit {
                return Err(failed(self.name, format!("no answer within {limit:?}")));
            }
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(|error| failed(self.name, error))?;
        }
        Ok(())
    }
}

pub(crate) fn millis(started: std::time::Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}
