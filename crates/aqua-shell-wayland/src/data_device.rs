use crate::formats::{Bytes, Encoded};
use crate::globals::Globals;
use crate::state::Shell;
use aqua_shell::services::ClipboardError;
use std::io::Write;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_v1::{
    self, ZwpPrimarySelectionDeviceV1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::{
    self, ZwpPrimarySelectionOfferV1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_source_v1::{
    self, ZwpPrimarySelectionSourceV1,
};

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Board {
    Clipboard,
    Primary,
}

impl Board {
    fn unsupported(self) -> ClipboardError {
        match self {
            Board::Clipboard => {
                ClipboardError::Unavailable("the compositor has no wl_data_device_manager")
            }
            Board::Primary => ClipboardError::Unsupported(
                "primary selection (the compositor has no zwp_primary_selection_device_manager_v1)",
            ),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct OfferData {
    mimes: Mutex<Vec<String>>,
}

impl OfferData {
    fn push(&self, mime: String) {
        self.mimes
            .lock()
            .expect("offer mime lists are never poisoned")
            .push(mime);
    }

    pub(crate) fn mimes(&self) -> Vec<String> {
        self.mimes
            .lock()
            .expect("offer mime lists are never poisoned")
            .clone()
    }
}

pub(crate) struct SourceData {
    board: Board,
    payload: Arc<Encoded>,
}

#[derive(Debug, Clone)]
pub(crate) enum Offer {
    Data(WlDataOffer),
    Primary(ZwpPrimarySelectionOfferV1),
}

impl Offer {
    pub(crate) fn mimes(&self) -> Vec<String> {
        let data = match self {
            Offer::Data(offer) => offer.data::<OfferData>(),
            Offer::Primary(offer) => offer.data::<OfferData>(),
        };
        data.map(OfferData::mimes).unwrap_or_default()
    }

    pub(crate) fn receive(&self, mime: &str, fd: BorrowedFd<'_>) {
        match self {
            Offer::Data(offer) => offer.receive(mime.to_owned(), fd),
            Offer::Primary(offer) => offer.receive(mime.to_owned(), fd),
        }
    }

    fn destroy(&self) {
        match self {
            Offer::Data(offer) => offer.destroy(),
            Offer::Primary(offer) => offer.destroy(),
        }
    }
}

enum Source {
    Data(WlDataSource),
    Primary(ZwpPrimarySelectionSourceV1),
}

impl Source {
    fn destroy(&self) {
        match self {
            Source::Data(source) => source.destroy(),
            Source::Primary(source) => source.destroy(),
        }
    }

    fn id(&self) -> wayland_client::backend::ObjectId {
        match self {
            Source::Data(source) => source.id(),
            Source::Primary(source) => source.id(),
        }
    }
}

enum Claim {
    Settling { generation: u64 },
    Settled,
}

struct Owned {
    source: Source,
    payload: Arc<Encoded>,
    marker: String,
    claim: Claim,
}

#[derive(Default)]
struct Selection {
    owned: Option<Owned>,
    offer: Option<Offer>,
}

pub(crate) enum Snapshot {
    Ours(Arc<Encoded>),
    Foreign(Offer),
    Empty,
}

impl Selection {
    fn snapshot(&self) -> Snapshot {
        match (&self.owned, &self.offer) {
            (Some(owned), _) => Snapshot::Ours(owned.payload.clone()),
            (None, Some(offer)) => Snapshot::Foreign(offer.clone()),
            (None, None) => Snapshot::Empty,
        }
    }

    fn replace_offer(&mut self, offer: Option<Offer>) {
        if let Some(old) = std::mem::replace(&mut self.offer, offer) {
            old.destroy();
        }
    }

    fn disown(&mut self) {
        if let Some(owned) = self.owned.take() {
            owned.source.destroy();
        }
    }

    fn claim(&mut self, owned: Owned) {
        self.disown();
        self.replace_offer(None);
        self.owned = Some(owned);
    }

    fn announced(&mut self, offer: Option<Offer>) {
        let echo = |owned: &Owned, offer: &Offer| offer.mimes().contains(&owned.marker);
        match (&self.owned, offer) {
            (Some(owned), Some(offer)) if echo(owned, &offer) => offer.destroy(),
            (
                Some(Owned {
                    claim: Claim::Settling { .. },
                    ..
                }),
                stale,
            ) => {
                if let Some(stale) = stale {
                    stale.destroy();
                }
            }
            (_, offer) => {
                self.disown();
                self.replace_offer(offer);
            }
        }
    }

    fn settled(&mut self, generation: u64) {
        if let Some(owned) = self.owned.as_mut()
            && matches!(owned.claim, Claim::Settling { generation: pending } if pending == generation)
        {
            owned.claim = Claim::Settled;
        }
    }

    fn is_settled(&self) -> bool {
        !matches!(
            self.owned,
            Some(Owned {
                claim: Claim::Settling { .. },
                ..
            })
        )
    }

    fn cancelled(&mut self, source: &wayland_client::backend::ObjectId) {
        if self
            .owned
            .as_ref()
            .is_some_and(|owned| owned.source.id() == *source)
        {
            self.disown();
        }
    }

    fn clear(&mut self) {
        self.disown();
        self.replace_offer(None);
    }
}

#[derive(Default)]
pub(crate) struct Transfer {
    data: Option<WlDataDevice>,
    primary: Option<ZwpPrimarySelectionDeviceV1>,
    clipboard: Selection,
    primary_selection: Selection,
}

impl Transfer {
    pub(crate) fn attach(&mut self, globals: &Globals, seat: &WlSeat, qh: &QueueHandle<Shell>) {
        if let (None, Some(manager)) = (&self.data, &globals.data_device) {
            self.data = Some(manager.get_data_device(seat, qh, ()));
        }
        if let (None, Some(manager)) = (&self.primary, &globals.primary_selection) {
            self.primary = Some(manager.get_device(seat, qh, ()));
        }
    }

    pub(crate) fn release(&mut self) {
        self.clipboard.clear();
        self.primary_selection.clear();
        if let Some(device) = self.data.take()
            && device.version() >= 2
        {
            device.release();
        }
        if let Some(device) = self.primary.take() {
            device.destroy();
        }
    }

    fn selection(&mut self, board: Board) -> &mut Selection {
        match board {
            Board::Clipboard => &mut self.clipboard,
            Board::Primary => &mut self.primary_selection,
        }
    }

    pub(crate) fn snapshot(&mut self, board: Board) -> Snapshot {
        self.selection(board).snapshot()
    }

    pub(crate) fn is_settled(&mut self, board: Board) -> bool {
        self.selection(board).is_settled()
    }

    pub(crate) fn has_device(&self, board: Board) -> bool {
        match board {
            Board::Clipboard => self.data.is_some(),
            Board::Primary => self.primary.is_some(),
        }
    }
}

fn marker() -> String {
    format!(
        "application/x-aqua-selection;owner={};id={}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

impl Shell {
    pub(crate) fn set_selection(
        &mut self,
        board: Board,
        payload: Option<Encoded>,
    ) -> Result<(), ClipboardError> {
        let serial = self
            .seat
            .serial
            .ok_or(ClipboardError::Unavailable("no input serial"))?;
        if !self.transfer.has_device(board) {
            return Err(board.unsupported());
        }
        let qh = self.qh.clone();
        let Some(payload) = payload else {
            match (board, &self.transfer.data, &self.transfer.primary) {
                (Board::Clipboard, Some(device), _) => device.set_selection(None, serial),
                (Board::Primary, _, Some(device)) => device.set_selection(None, serial),
                _ => {}
            }
            self.transfer.selection(board).clear();
            return Ok(());
        };
        let payload = Arc::new(payload);
        let marker = marker();
        let data = SourceData {
            board,
            payload: payload.clone(),
        };
        let mimes = payload
            .formats
            .iter()
            .map(|(mime, _)| (*mime).to_owned())
            .chain(std::iter::once(marker.clone()));
        let source = match board {
            Board::Clipboard => {
                let manager = self
                    .globals
                    .data_device
                    .as_ref()
                    .ok_or_else(|| board.unsupported())?;
                let source = manager.create_data_source(&qh, data);
                mimes.for_each(|mime| source.offer(mime));
                if let Some(device) = &self.transfer.data {
                    device.set_selection(Some(&source), serial);
                }
                Source::Data(source)
            }
            Board::Primary => {
                let manager = self
                    .globals
                    .primary_selection
                    .as_ref()
                    .ok_or_else(|| board.unsupported())?;
                let source = manager.create_source(&qh, data);
                mimes.for_each(|mime| source.offer(mime));
                if let Some(device) = &self.transfer.primary {
                    device.set_selection(Some(&source), serial);
                }
                Source::Primary(source)
            }
        };
        let generation = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        self.conn.display().sync(&qh, Settle { board, generation });
        self.transfer.selection(board).claim(Owned {
            source,
            payload,
            marker,
            claim: Claim::Settling { generation },
        });
        Ok(())
    }
}

fn serve(payload: &Encoded, mime: &str, fd: OwnedFd) {
    let Some(bytes) = payload.bytes(mime) else {
        return;
    };
    let write = move |bytes: Bytes, fd: OwnedFd| {
        let mut file = std::fs::File::from(fd);
        if let Err(error) = file.write_all(&bytes)
            && error.kind() != std::io::ErrorKind::BrokenPipe
        {
            eprintln!(
                "aqua: clipboard: writing {} bytes failed: {error}",
                bytes.len()
            );
        }
    };
    let spawned = std::thread::Builder::new()
        .name("aqua-selection-send".into())
        .spawn(move || write(bytes, fd));
    if let Err(error) = spawned {
        eprintln!("aqua: clipboard: no thread to serve a paste: {error}");
    }
}

pub(crate) struct Settle {
    board: Board,
    generation: u64,
}

impl Dispatch<WlCallback, Settle> for Shell {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        settle: &Settle,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state
                .transfer
                .selection(settle.board)
                .settled(settle.generation);
        }
    }
}

impl Dispatch<WlDataDevice, ()> for Shell {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Selection { id } => {
                state.transfer.clipboard.announced(id.map(Offer::Data))
            }
            wl_data_device::Event::Enter {
                serial,
                surface,
                x,
                y,
                id,
            } => crate::drop::entered(state, serial, &surface, (x, y), id),
            wl_data_device::Event::Motion { x, y, .. } => crate::drop::moved(state, (x, y)),
            wl_data_device::Event::Leave => crate::drop::left(state),
            wl_data_device::Event::Drop => crate::drop::dropped(state),
            _ => {}
        }
    }

    event_created_child!(Shell, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, OfferData::default()),
    ]);
}

impl Dispatch<WlDataOffer, OfferData> for Shell {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        data: &OfferData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            data.push(mime_type);
        }
    }
}

impl Dispatch<WlDataSource, SourceData> for Shell {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        data: &SourceData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => serve(&data.payload, &mime_type, fd),
            wl_data_source::Event::Cancelled => {
                state.transfer.selection(data.board).cancelled(&source.id())
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpPrimarySelectionDeviceV1, ()> for Shell {
    fn event(
        state: &mut Self,
        _: &ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_device_v1::Event::Selection { id } = event {
            state
                .transfer
                .primary_selection
                .announced(id.map(Offer::Primary));
        }
    }

    event_created_child!(Shell, ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (ZwpPrimarySelectionOfferV1, OfferData::default()),
    ]);
}

impl Dispatch<ZwpPrimarySelectionOfferV1, OfferData> for Shell {
    fn event(
        _: &mut Self,
        _: &ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        data: &OfferData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event {
            data.push(mime_type);
        }
    }
}

impl Dispatch<ZwpPrimarySelectionSourceV1, SourceData> for Shell {
    fn event(
        state: &mut Self,
        source: &ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        data: &SourceData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_source_v1::Event::Send { mime_type, fd } => {
                serve(&data.payload, &mime_type, fd)
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                state.transfer.selection(data.board).cancelled(&source.id())
            }
            _ => {}
        }
    }
}
