use aqua_platform::geometry::{LogicalPosition, LogicalSize};
use aqua_platform::services::{
    set_ime_cursor_area, Clipboard, ClipboardError, ClipboardImage, ImeAreaWindow, ImeCursorArea,
    InProcessDnD, MemoryClipboard,
};
use std::cell::RefCell;
use std::path::PathBuf;

#[test]
fn clipboard_replaces_formats_and_clears_all_content() {
    let mut clipboard = MemoryClipboard::new();
    clipboard.set_text("first".into()).unwrap();
    assert_eq!(clipboard.get_text().unwrap().as_deref(), Some("first"));
    let image = ClipboardImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 128]).unwrap();
    clipboard.set_image(image.clone()).unwrap();
    assert_eq!(clipboard.get_text().unwrap(), None);
    assert_eq!(clipboard.get_image().unwrap(), Some(image));
    let files = vec![PathBuf::from("a b.png"), PathBuf::from("多言語.txt")];
    clipboard.set_files(files.clone()).unwrap();
    assert_eq!(clipboard.get_image().unwrap(), None);
    assert_eq!(clipboard.get_files().unwrap(), Some(files));
    clipboard.clear().unwrap();
    assert_eq!(clipboard.get_text().unwrap(), None);
    assert_eq!(clipboard.get_image().unwrap(), None);
    assert_eq!(clipboard.get_files().unwrap(), None);
    clipboard.set_text("replaced".into()).unwrap();
    clipboard.set_files(Vec::new()).unwrap();
    assert_eq!(clipboard.get_text().unwrap(), None);
}

#[test]
fn clipboard_image_rejects_invalid_dimensions_and_byte_lengths() {
    for (width, height, bytes) in [
        (0, 1, 0),
        (1, 0, 0),
        (2, 2, 15),
        (2, 2, 17),
        (usize::MAX, 2, 0),
    ] {
        assert_eq!(
            ClipboardImage::new(width, height, vec![0; bytes]),
            Err(ClipboardError::InvalidImage)
        );
    }
    let image = ClipboardImage::new(1, 1, vec![10, 20, 30, 40]).unwrap();
    assert_eq!(
        (image.width(), image.height(), image.rgba()),
        (1, 1, [10, 20, 30, 40].as_slice())
    );
}

type AreaRecord = Option<((f64, f64), (f64, f64))>;

struct FakeWindow(RefCell<AreaRecord>);

impl ImeAreaWindow for FakeWindow {
    fn set_area(&self, origin: (f64, f64), size: (f64, f64)) {
        *self.0.borrow_mut() = Some((origin, size));
    }
}

#[test]
fn ime_area_rejects_non_finite_values_without_forwarding() {
    let window = FakeWindow(RefCell::new(None));
    for values in [
        [f64::NAN, 0.0, 2.0, 16.0],
        [0.0, f64::INFINITY, 2.0, 16.0],
        [0.0, 0.0, f64::INFINITY, 16.0],
        [0.0, 0.0, 2.0, f64::NAN],
        [0.0, 0.0, -1.0, 16.0],
        [0.0, 0.0, 2.0, -1.0],
    ] {
        assert!(!set_ime_cursor_area(
            &window,
            ImeCursorArea {
                origin: LogicalPosition {
                    x: values[0],
                    y: values[1]
                },
                size: LogicalSize::new(values[2], values[3]),
            }
        ));
        assert!(window.0.borrow().is_none());
    }
    assert!(set_ime_cursor_area(
        &window,
        ImeCursorArea {
            origin: LogicalPosition { x: -4.0, y: 12.0 },
            size: LogicalSize::new(2.0, 16.0),
        }
    ));
    assert_eq!(*window.0.borrow(), Some(((-4.0, 12.0), (2.0, 16.0))));
}

#[test]
fn in_process_payload_replacement_is_per_format() {
    let mut dnd = InProcessDnD::default();
    dnd.offer("text/plain", b"first".to_vec());
    dnd.offer("image/png", b"image".to_vec());
    dnd.offer("text/plain", b"replacement".to_vec());
    assert_eq!(dnd.get("text/plain"), Some(b"replacement".as_slice()));
    assert_eq!(dnd.get("image/png"), Some(b"image".as_slice()));
}
