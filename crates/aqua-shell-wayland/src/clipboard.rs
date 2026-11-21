use crate::data_device::{Board, Offer, Snapshot};
use crate::formats::{self, Content, Encoded};
use crate::state;
use aqua_shell::services::{Clipboard, ClipboardError, ClipboardImage};
use rustix::event::{poll, PollFd, PollFlags};
use std::marker::PhantomData;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use wayland_client::backend::WaylandError;
use wayland_client::Connection;

const READ_LIMIT: Duration = Duration::from_millis(500);
const SETTLE_LIMIT: Duration = Duration::from_millis(500);
const DISCONNECTED: ClipboardError =
    ClipboardError::Unavailable("the Wayland shell is not connected on this thread");

fn os(what: &str, error: impl std::fmt::Display) -> ClipboardError {
    ClipboardError::Os(format!("{what}: {error}"))
}

pub(crate) fn receive(
    conn: &Connection,
    offer: &Offer,
    mime: &str,
    limit: Duration,
) -> Result<Vec<u8>, ClipboardError> {
    let (reader, writer) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
        .map_err(|error| os("pipe", error))?;
    offer.receive(mime, writer.as_fd());
    drop(writer);
    match conn.flush() {
        Ok(()) => {}
        Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(error) => return Err(os("wl_display_flush", error)),
    }
    let timeout = crate::clock::timespec(limit);
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut fds = [PollFd::new(&reader, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => {
                return Err(ClipboardError::Unavailable(
                    "the selection owner did not answer in time",
                ))
            }
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(os("poll", error)),
        }
        match rustix::io::read(&reader, &mut chunk) {
            Ok(0) => return Ok(bytes),
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => {}
            Err(error) => return Err(os("reading the selection", error)),
        }
    }
}

fn connection() -> Result<Connection, ClipboardError> {
    state::try_with(|shell| shell.conn.clone()).ok_or(DISCONNECTED)
}

fn snapshot(board: Board) -> Result<Snapshot, ClipboardError> {
    state::try_with(|shell| shell.transfer.snapshot(board)).ok_or(DISCONNECTED)
}

fn first_offered(offer: &Offer, wanted: &[&str]) -> Option<String> {
    let offered = offer.mimes();
    wanted
        .iter()
        .find(|mime| offered.iter().any(|candidate| candidate == *mime))
        .map(|mime| (*mime).to_owned())
}

fn read<T>(
    board: Board,
    wanted: &[&str],
    ours: impl FnOnce(&Content) -> Option<T>,
    theirs: impl FnOnce(Vec<u8>) -> Result<Option<T>, ClipboardError>,
) -> Result<Option<T>, ClipboardError> {
    crate::looping::refresh().map_err(|error| os("reading compositor events", error))?;
    match snapshot(board)? {
        Snapshot::Empty => Ok(None),
        Snapshot::Ours(payload) => Ok(ours(&payload.content)),
        Snapshot::Foreign(offer) => match first_offered(&offer, wanted) {
            None => Ok(None),
            Some(mime) => theirs(receive(&connection()?, &offer, &mime, READ_LIMIT)?),
        },
    }
}

fn write(board: Board, content: Option<Content>) -> Result<(), ClipboardError> {
    let payload = content.map(Encoded::new).transpose()?;
    state::try_with(|shell| shell.set_selection(board, payload)).ok_or(DISCONNECTED)??;
    let settled = crate::looping::wait_until(SETTLE_LIMIT, "wl_display.sync", || {
        state::try_with(|shell| shell.transfer.is_settled(board))
    });
    if let Err(error) = settled {
        eprintln!("aqua: clipboard: {error}; keeping the copy as ours");
    }
    Ok(())
}

fn connect() -> Result<(), ClipboardError> {
    state::ensure().map_err(|error| os("connecting to the compositor", error))
}

pub struct SystemClipboard {
    _thread_bound: PhantomData<Rc<()>>,
}

impl SystemClipboard {
    pub fn new() -> Result<Self, ClipboardError> {
        connect()?;
        match state::with(|shell| shell.globals.data_device.is_some()) {
            true => Ok(Self {
                _thread_bound: PhantomData,
            }),
            false => Err(ClipboardError::Unavailable(
                "the compositor has no wl_data_device_manager",
            )),
        }
    }
}

impl Clipboard for SystemClipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        read(
            Board::Clipboard,
            formats::TEXT,
            |content| match content {
                Content::Text(text) => Some(text.clone()),
                _ => None,
            },
            |bytes| Ok(Some(formats::parse_text(bytes))),
        )
    }

    fn set_text(&mut self, text: String) -> Result<(), ClipboardError> {
        write(Board::Clipboard, Some(Content::Text(text)))
    }

    fn clear(&mut self) -> Result<(), ClipboardError> {
        write(Board::Clipboard, None)
    }

    fn get_image(&mut self) -> Result<Option<ClipboardImage>, ClipboardError> {
        read(
            Board::Clipboard,
            formats::IMAGES,
            |content| match content {
                Content::Image(image) => Some(image.clone()),
                _ => None,
            },
            |bytes| formats::decode_png(&bytes).map(Some),
        )
    }

    fn set_image(&mut self, image: ClipboardImage) -> Result<(), ClipboardError> {
        write(Board::Clipboard, Some(Content::Image(image)))
    }

    fn get_files(&mut self) -> Result<Option<Vec<PathBuf>>, ClipboardError> {
        read(
            Board::Clipboard,
            formats::FILES,
            |content| match content {
                Content::Files(files) => Some(files.clone()),
                _ => None,
            },
            |bytes| {
                let files = formats::parse_files(&bytes);
                Ok((!files.is_empty()).then_some(files))
            },
        )
    }

    fn set_files(&mut self, files: Vec<PathBuf>) -> Result<(), ClipboardError> {
        match files.is_empty() {
            true => self.clear(),
            false => write(Board::Clipboard, Some(Content::Files(files))),
        }
    }
}

pub struct PrimarySelection {
    _thread_bound: PhantomData<Rc<()>>,
}

impl PrimarySelection {
    pub fn new() -> Result<Self, ClipboardError> {
        connect()?;
        match state::with(|shell| shell.globals.primary_selection.is_some()) {
            true => Ok(Self {
                _thread_bound: PhantomData,
            }),
            false => Err(ClipboardError::Unsupported(
                "primary selection (the compositor has no zwp_primary_selection_device_manager_v1)",
            )),
        }
    }
}

impl Clipboard for PrimarySelection {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        read(
            Board::Primary,
            formats::TEXT,
            |content| match content {
                Content::Text(text) => Some(text.clone()),
                _ => None,
            },
            |bytes| Ok(Some(formats::parse_text(bytes))),
        )
    }

    fn set_text(&mut self, text: String) -> Result<(), ClipboardError> {
        write(Board::Primary, Some(Content::Text(text)))
    }

    fn clear(&mut self) -> Result<(), ClipboardError> {
        write(Board::Primary, None)
    }
}
