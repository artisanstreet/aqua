use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use windows::Win32::System::Performance::QueryPerformanceCounter;

const FLUSH_AT: usize = 128;

static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

thread_local! {
    static LAST_WAIT: Cell<i64> = const { Cell::new(0) };
    static ROWS: RefCell<Vec<(i64, i64, i64)>> = const { RefCell::new(Vec::new()) };
}

fn path() -> Option<&'static PathBuf> {
    PATH.get_or_init(|| std::env::var_os("AQUA_WM_NULL_TRACE").map(PathBuf::from))
        .as_ref()
}

pub(crate) fn enabled() -> bool {
    path().is_some()
}

pub(crate) fn qpc() -> i64 {
    let mut ticks = 0i64;
    let _ = unsafe { QueryPerformanceCounter(&mut ticks) };
    ticks
}

pub(crate) fn waited() {
    if enabled() {
        LAST_WAIT.set(qpc());
    }
}

pub(crate) fn record(entry: i64, exit: i64) {
    let wait = LAST_WAIT.get();
    ROWS.with(|rows| rows.borrow_mut().push((wait, entry, exit)));
}

pub(crate) fn flush_if_due() {
    let Some(path) = path() else {
        return;
    };
    let rows = ROWS.with(|rows| {
        let mut rows = rows.borrow_mut();
        (rows.len() >= FLUSH_AT).then(|| std::mem::take(&mut *rows))
    });
    let Some(rows) = rows else {
        return;
    };
    let text: String = rows
        .iter()
        .map(|(wait, entry, exit)| format!("{wait},{entry},{exit}\n"))
        .collect();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(text.as_bytes());
    }
}
