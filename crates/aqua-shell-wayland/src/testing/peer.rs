use super::client::{failed, Client};
use super::HarnessError;
use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags};
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::Dispatch;

pub(crate) struct Peer<C: Send + 'static> {
    commands: mpsc::Sender<Option<C>>,
    wake: Arc<OwnedFd>,
    thread: Option<JoinHandle<()>>,
}

pub(crate) trait Serve:
    Sized + Send + Dispatch<WlRegistry, GlobalListContents> + 'static
{
    type Command: Send + 'static;

    fn start(client: &mut Client<Self>) -> Result<(), HarnessError>;
    fn command(client: &mut Client<Self>, command: Self::Command);
}

fn wait(client: &Client<impl Serve>, wake: &OwnedFd) -> Result<bool, HarnessError> {
    client.flush()?;
    let Some(guard) = client.queue.prepare_read() else {
        return Ok(false);
    };
    let mut fds = [
        PollFd::from_borrowed_fd(guard.connection_fd(), PollFlags::IN),
        PollFd::new(wake, PollFlags::IN),
    ];
    match poll(&mut fds, None) {
        Ok(_) | Err(rustix::io::Errno::INTR) => {}
        Err(error) => return Err(failed(client.name, error)),
    }
    let (socket, woken) = (fds[0].revents(), fds[1].revents());
    if socket.intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP) {
        guard.read().map_err(|error| failed(client.name, error))?;
    }
    Ok(woken.contains(PollFlags::IN))
}

impl<C: Send + 'static> Peer<C> {
    pub(crate) fn spawn<S: Serve<Command = C>>(
        display: PathBuf,
        name: &'static str,
        state: S,
    ) -> Result<Self, HarnessError> {
        let wake = Arc::new(
            eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
                .map_err(|error| failed(name, error))?,
        );
        let (commands, inbox) = mpsc::channel::<Option<C>>();
        let (started, ready) = mpsc::channel();
        let thread_wake = wake.clone();
        let thread = std::thread::Builder::new()
            .name(format!("aqua-test-{name}"))
            .spawn(move || {
                let mut client = match Client::connect(&display, name, state) {
                    Ok(client) => client,
                    Err(error) => return drop(started.send(Err(error))),
                };
                if let Err(error) = S::start(&mut client) {
                    return drop(started.send(Err(error)));
                }
                let _ = started.send(Ok(()));
                loop {
                    if let Err(error) = client.queue.dispatch_pending(&mut client.state) {
                        eprintln!("aqua test {name}: {error}");
                        return;
                    }
                    match wait(&client, &thread_wake) {
                        Ok(false) => continue,
                        Ok(true) => {}
                        Err(error) => {
                            eprintln!("aqua test {name}: {error}");
                            return;
                        }
                    }
                    let mut counter = [0u8; 8];
                    let _ = rustix::io::read(thread_wake.as_fd(), &mut counter);
                    for command in inbox.try_iter() {
                        match command {
                            Some(command) => S::command(&mut client, command),
                            None => return,
                        }
                    }
                }
            })
            .map_err(|error| failed(name, error))?;
        ready
            .recv()
            .map_err(|_| failed(name, "the peer thread died while starting"))??;
        Ok(Self {
            commands,
            wake,
            thread: Some(thread),
        })
    }

    pub(crate) fn send(&self, command: C) {
        let _ = self.commands.send(Some(command));
        let _ = rustix::io::write(self.wake.as_fd(), &1u64.to_ne_bytes());
    }
}

impl<C: Send + 'static> Drop for Peer<C> {
    fn drop(&mut self) {
        let _ = self.commands.send(None);
        let _ = rustix::io::write(self.wake.as_fd(), &1u64.to_ne_bytes());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) fn read_pipe(reader: OwnedFd) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::fs::File::from(reader), &mut bytes)?;
    Ok(bytes)
}

pub(crate) fn write_pipe(fd: OwnedFd, bytes: Vec<u8>) {
    std::thread::spawn(move || {
        let _ = std::io::Write::write_all(&mut std::fs::File::from(fd), &bytes);
    });
}
