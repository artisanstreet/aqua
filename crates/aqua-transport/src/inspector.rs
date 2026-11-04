use std::fmt::Write as _;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameRecord {
    pub frame: u64,
    pub extent: (u32, u32),
    pub passes: u32,
    pub instances: u32,
    pub barriers: u32,
    pub transient_bytes: u64,
    pub commands: u32,
    pub unsupported_items: u32,
    pub approximated_items: u32,
    pub glyphs: u32,
    pub trimmed_entries: u64,
    pub cpu_micros: u64,
    pub gpu_micros: Option<u64>,
    pub present_queued_micros: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InspectorSnapshot {
    pub frames_presented: u64,
    pub frames_failed: u64,
    pub device_losses: u32,
    pub scene_items: usize,
    pub tracks: usize,
    pub last: Option<FrameRecord>,
    pub last_error: Option<String>,
    pub tree_json: Option<String>,
}

struct Shared {
    state: Mutex<(u64, InspectorSnapshot)>,
    changed: Condvar,
}

#[derive(Clone)]
pub struct Inspector {
    shared: Arc<Shared>,
}

impl Default for Inspector {
    fn default() -> Self {
        Self::new()
    }
}

impl Inspector {
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new((0, InspectorSnapshot::default())),
                changed: Condvar::new(),
            }),
        }
    }

    pub fn update(&self, change: impl FnOnce(&mut InspectorSnapshot)) {
        let mut guard = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        change(&mut guard.1);
        guard.0 += 1;
        drop(guard);
        self.shared.changed.notify_all();
    }

    #[must_use]
    pub fn snapshot(&self) -> InspectorSnapshot {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .clone()
    }

    fn wait_for_change(&self, seen: u64, timeout: Duration) -> (u64, InspectorSnapshot) {
        let guard = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, _) = self
            .shared
            .changed
            .wait_timeout_while(guard, timeout, |state| state.0 == seen)
            .unwrap_or_else(|e| e.into_inner());
        (guard.0, guard.1.clone())
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "null".to_owned(), |v| v.to_string())
}

#[must_use]
pub fn stats_json(snapshot: &InspectorSnapshot) -> String {
    let last = snapshot.last.as_ref().map_or_else(
        || "null".to_owned(),
        |record| {
            format!(
                "{{\"frame\":{},\"width\":{},\"height\":{},\"passes\":{},\"instances\":{},\"barriers\":{},\"transient_bytes\":{},\"commands\":{},\"unsupported_items\":{},\"approximated_items\":{},\"glyphs\":{},\"trimmed_entries\":{},\"cpu_micros\":{},\"gpu_micros\":{},\"present_queued_micros\":{}}}",
                record.frame,
                record.extent.0,
                record.extent.1,
                record.passes,
                record.instances,
                record.barriers,
                record.transient_bytes,
                record.commands,
                record.unsupported_items,
                record.approximated_items,
                record.glyphs,
                record.trimmed_entries,
                record.cpu_micros,
                optional(record.gpu_micros),
                record.present_queued_micros,
            )
        },
    );
    let error = snapshot
        .last_error
        .as_deref()
        .map_or_else(|| "null".to_owned(), escape);
    format!(
        "{{\"frames_presented\":{},\"frames_failed\":{},\"device_losses\":{},\"scene_items\":{},\"tracks\":{},\"last\":{},\"last_error\":{}}}",
        snapshot.frames_presented,
        snapshot.frames_failed,
        snapshot.device_losses,
        snapshot.scene_items,
        snapshot.tracks,
        last,
        error,
    )
}

#[must_use]
pub fn tree_json(snapshot: &InspectorSnapshot) -> String {
    format!(
        "{{\"tree\":{}}}",
        snapshot.tree_json.as_deref().unwrap_or("null")
    )
}

#[derive(Debug)]
pub enum InspectorError {
    NotLoopback(SocketAddr),
    Io(io::Error),
}

impl std::fmt::Display for InspectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InspectorError::NotLoopback(addr) => {
                write!(f, "inspector refuses non-loopback address {addr}")
            }
            InspectorError::Io(error) => write!(f, "inspector socket: {error}"),
        }
    }
}

impl std::error::Error for InspectorError {}

impl From<io::Error> for InspectorError {
    fn from(error: io::Error) -> Self {
        InspectorError::Io(error)
    }
}

pub struct InspectorServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl InspectorServer {
    pub fn bind(inspector: Inspector, addr: SocketAddr) -> Result<Self, InspectorError> {
        if !addr.ip().is_loopback() {
            return Err(InspectorError::NotLoopback(addr));
        }
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = thread::spawn(move || accept_loop(listener, inspector, flag));
        Ok(Self {
            addr,
            stop,
            handle: Some(handle),
        })
    }

    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for InspectorServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn accept_loop(listener: TcpListener, inspector: Inspector, stop: Arc<AtomicBool>) {
    let mut connections: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let inspector = inspector.clone();
                let stop = Arc::clone(&stop);
                connections.push(thread::spawn(move || {
                    let _ = serve(stream, inspector, stop);
                }));
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => break,
        }
        connections.retain(|handle| !handle.is_finished());
    }
    for handle in connections {
        let _ = handle.join();
    }
}

fn serve(stream: TcpStream, inspector: Inspector, stop: Arc<AtomicBool>) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(POLL))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    while !stop.load(Ordering::Acquire) {
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                let command = line.trim().to_owned();
                line.clear();
                match command.as_str() {
                    "stats" => writeln!(writer, "{}", stats_json(&inspector.snapshot()))?,
                    "tree" => writeln!(writer, "{}", tree_json(&inspector.snapshot()))?,
                    "watch" => return watch(&mut writer, &inspector, &stop),
                    "quit" => return Ok(()),
                    "" => {}
                    _ => writeln!(writer, "{{\"error\":\"unknown command\"}}")?,
                }
                writer.flush()?;
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn watch(writer: &mut TcpStream, inspector: &Inspector, stop: &AtomicBool) -> io::Result<()> {
    let (mut seen, snapshot) = inspector.wait_for_change(u64::MAX, Duration::ZERO);
    writeln!(writer, "{}", stats_json(&snapshot))?;
    while !stop.load(Ordering::Acquire) {
        let (generation, snapshot) = inspector.wait_for_change(seen, POLL);
        if generation != seen {
            seen = generation;
            writeln!(writer, "{}", stats_json(&snapshot))?;
            writer.flush()?;
        }
    }
    Ok(())
}
