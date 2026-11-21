use super::client::{failed, Client};
use super::compositor::Compositor;
use super::peer::{read_pipe, write_pipe, Peer, Serve};
use super::HarnessError;
use std::os::fd::AsFd;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{
    delegate_noop, event_created_child, Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::{
    self, ExtDataControlDeviceV1,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::{
    self, ExtDataControlOfferV1,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::{
    self, ExtDataControlSourceV1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Clipboard,
    Primary,
}

type Mimes = Arc<Mutex<Vec<String>>>;
type Entries = Arc<Vec<(String, Vec<u8>)>>;
pub type Reply = mpsc::Receiver<Result<Option<Vec<u8>>, HarnessError>>;

#[derive(Debug, Clone, Default)]
struct Seen {
    clipboard: Option<Vec<String>>,
    primary: Option<Vec<String>>,
}

pub struct Control {
    manager: Option<ExtDataControlManagerV1>,
    device: Option<ExtDataControlDeviceV1>,
    clipboard: Option<ExtDataControlOfferV1>,
    primary: Option<ExtDataControlOfferV1>,
    sources: Vec<ExtDataControlSourceV1>,
    seen: Arc<Mutex<Seen>>,
}

pub enum Command {
    Read(
        Selection,
        String,
        mpsc::Sender<Result<Option<Vec<u8>>, HarnessError>>,
    ),
    Offer(Selection, Entries),
}

pub struct DataControl {
    peer: Peer<Command>,
    seen: Arc<Mutex<Seen>>,
}

fn mimes_of(offer: &ExtDataControlOfferV1) -> Vec<String> {
    offer
        .data::<Mimes>()
        .map(|mimes| mimes.lock().expect("mime lists are never poisoned").clone())
        .unwrap_or_default()
}

impl Control {
    fn record(&mut self, which: Selection, offer: Option<ExtDataControlOfferV1>) {
        let mimes = offer.as_ref().map(mimes_of);
        let slot = match which {
            Selection::Clipboard => &mut self.clipboard,
            Selection::Primary => &mut self.primary,
        };
        if let Some(old) = std::mem::replace(slot, offer) {
            old.destroy();
        }
        let mut seen = self
            .seen
            .lock()
            .expect("seen selections are never poisoned");
        match which {
            Selection::Clipboard => seen.clipboard = mimes,
            Selection::Primary => seen.primary = mimes,
        }
    }
}

impl Serve for Control {
    type Command = Command;

    fn start(client: &mut Client<Self>) -> Result<(), HarnessError> {
        let seat: WlSeat = client.bind(1, ())?;
        let manager: ExtDataControlManagerV1 = client.bind(1, ())?;
        let device = manager.get_data_device(&seat, &client.handle(), ());
        client.state.manager = Some(manager);
        client.state.device = Some(device);
        client.roundtrip()
    }

    fn command(client: &mut Client<Self>, command: Command) {
        match command {
            Command::Read(which, mime, reply) => {
                let offer = match which {
                    Selection::Clipboard => client.state.clipboard.clone(),
                    Selection::Primary => client.state.primary.clone(),
                };
                let Some(offer) = offer.filter(|offer| mimes_of(offer).contains(&mime)) else {
                    let _ = reply.send(Ok(None));
                    return;
                };
                let pipe = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC);
                let (reader, writer) = match pipe {
                    Ok(pipe) => pipe,
                    Err(error) => return drop(reply.send(Err(failed(client.name, error)))),
                };
                offer.receive(mime, writer.as_fd());
                drop(writer);
                if let Err(error) = client.flush() {
                    return drop(reply.send(Err(error)));
                }
                let name = client.name;
                std::thread::spawn(move || {
                    let _ = reply.send(
                        read_pipe(reader)
                            .map(Some)
                            .map_err(|error| failed(name, error)),
                    );
                });
            }
            Command::Offer(which, entries) => {
                let Some(manager) = client.state.manager.clone() else {
                    return;
                };
                let source = manager.create_data_source(&client.handle(), entries.clone());
                entries
                    .iter()
                    .for_each(|(mime, _)| source.offer(mime.clone()));
                if let Some(device) = &client.state.device {
                    match which {
                        Selection::Clipboard => device.set_selection(Some(&source)),
                        Selection::Primary => device.set_primary_selection(Some(&source)),
                    }
                }
                client.state.sources.push(source);
            }
        }
    }
}

impl DataControl {
    pub fn connect(compositor: &Compositor) -> Result<Self, HarnessError> {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let control = Control {
            manager: None,
            device: None,
            clipboard: None,
            primary: None,
            sources: Vec::new(),
            seen: seen.clone(),
        };
        let peer = Peer::spawn(compositor.display().to_owned(), "data control", control)?;
        Ok(Self { peer, seen })
    }

    pub fn offered(&self, which: Selection) -> Option<Vec<String>> {
        let seen = self
            .seen
            .lock()
            .expect("seen selections are never poisoned");
        match which {
            Selection::Clipboard => seen.clipboard.clone(),
            Selection::Primary => seen.primary.clone(),
        }
    }

    pub fn read(&self, which: Selection, mime: &str) -> Reply {
        let (reply, answer) = mpsc::channel();
        self.peer.send(Command::Read(which, mime.to_owned(), reply));
        answer
    }

    pub fn offer(&self, which: Selection, entries: Vec<(String, Vec<u8>)>) {
        self.peer.send(Command::Offer(which, Arc::new(entries)));
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Control {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for Control {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::Selection { id } => {
                state.record(Selection::Clipboard, id)
            }
            ext_data_control_device_v1::Event::PrimarySelection { id } => {
                state.record(Selection::Primary, id)
            }
            _ => {}
        }
    }

    event_created_child!(Control, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, Mimes::default()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, Mimes> for Control {
    fn event(
        _: &mut Self,
        _: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        mimes: &Mimes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            mimes
                .lock()
                .expect("mime lists are never poisoned")
                .push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, Entries> for Control {
    fn event(
        state: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        entries: &Entries,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                if let Some((_, bytes)) = entries.iter().find(|(mime, _)| *mime == mime_type) {
                    write_pipe(fd, bytes.clone());
                }
            }
            ext_data_control_source_v1::Event::Cancelled => {
                state.sources.retain(|kept| kept != source);
                source.destroy();
            }
            _ => {}
        }
    }
}

delegate_noop!(Control: ignore WlSeat);
delegate_noop!(Control: ExtDataControlManagerV1);
