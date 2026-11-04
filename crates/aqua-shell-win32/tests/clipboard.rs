#![cfg(windows)]

use aqua_shell_win32::{clipboard, Window, WindowOptions};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};

const GDI_BITMAP: u32 = 2;
const GDI_METAFILE: u32 = 3;
const GDI_PALETTE: u32 = 9;
const GDI_ENHMETAFILE: u32 = 14;
const OWNER_DISPLAY: u32 = 0x80;
const PRIVATE_RANGE: std::ops::RangeInclusive<u32> = 0x200..=0x2FF;
const DISPLAY_RANGE: std::ops::RangeInclusive<u32> = 0x81..=0x8E;
const SYNTHESIZED_FROM_DIB: [u32; 2] = [GDI_BITMAP, GDI_PALETTE];

struct Snapshot {
    formats: Vec<(u32, Vec<u8>)>,
}

fn open_clipboard(owner: HWND) -> bool {
    (0..200).any(|_| {
        unsafe { OpenClipboard(Some(owner)) }.is_ok() || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            false
        }
    })
}

fn snapshot(owner: HWND) -> Option<Snapshot> {
    if !open_clipboard(owner) {
        return None;
    }
    let mut ids = Vec::new();
    let mut format = 0;
    loop {
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        ids.push(format);
    }
    let has_dib = ids.iter().any(|f| *f == 8 || *f == 17);
    let mut formats = Vec::new();
    let mut restorable = true;
    for id in ids {
        let skipped = SYNTHESIZED_FROM_DIB.contains(&id) && has_dib;
        let unrestorable = matches!(id, GDI_METAFILE | GDI_ENHMETAFILE | OWNER_DISPLAY)
            || DISPLAY_RANGE.contains(&id)
            || PRIVATE_RANGE.contains(&id)
            || (SYNTHESIZED_FROM_DIB.contains(&id) && !has_dib);
        match (skipped, unrestorable) {
            (true, _) => {}
            (false, true) => restorable = false,
            (false, false) => match unsafe { GetClipboardData(id) } {
                Ok(handle) => {
                    let global = HGLOBAL(handle.0);
                    let size = unsafe { GlobalSize(global) };
                    let ptr = unsafe { GlobalLock(global) } as *const u8;
                    match ptr.is_null() {
                        true => restorable = false,
                        false => {
                            let bytes = unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec();
                            let _ = unsafe { GlobalUnlock(global) };
                            formats.push((id, bytes));
                        }
                    }
                }
                Err(_) => restorable = false,
            },
        }
    }
    let _ = unsafe { CloseClipboard() };
    restorable.then_some(Snapshot { formats })
}

fn restore(owner: HWND, snapshot: &Snapshot) {
    assert!(open_clipboard(owner), "clipboard busy while restoring");
    let _ = unsafe { EmptyClipboard() };
    for (id, bytes) in &snapshot.formats {
        let global = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }.expect("alloc");
        let ptr = unsafe { GlobalLock(global) } as *mut u8;
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
        let _ = unsafe { GlobalUnlock(global) };
        if unsafe { SetClipboardData(*id, Some(HANDLE(global.0))) }.is_err() {
            let _ = unsafe { GlobalFree(Some(global)) };
        }
    }
    let _ = unsafe { CloseClipboard() };
}

#[test]
fn text_round_trips_and_the_previous_clipboard_comes_back() {
    let owner = Window::create(&WindowOptions {
        visible: false,
        ..Default::default()
    })
    .expect("owner");
    let hwnd = HWND(owner.raw_hwnd() as *mut _);
    let Some(saved) = snapshot(hwnd) else {
        eprintln!(
            "skipped: clipboard is denied or holds formats that cannot be restored byte for byte"
        );
        owner.destroy();
        return;
    };
    let first = clipboard::sequence();
    let samples = [
        "plain".to_owned(),
        "emoji \u{1F600} and\r\nnewlines\ttabs".to_owned(),
        String::new(),
        "x".repeat(1 << 20),
    ];
    let outcome = std::panic::catch_unwind(|| {
        for sample in &samples {
            clipboard::set_text(owner, sample).expect("set");
            assert_eq!(clipboard::get_text().expect("get").as_ref(), Some(sample));
            assert!(!clipboard::is_empty());
        }
        assert!(clipboard::sequence() > first);
    });
    restore(hwnd, &saved);
    let after = snapshot(hwnd).expect("restored snapshot");
    assert_eq!(after.formats, saved.formats, "clipboard was not restored");
    owner.destroy();
    outcome.expect("roundtrip");
}
