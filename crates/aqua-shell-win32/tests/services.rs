use aqua_shell_win32::services::{Clipboard, ClipboardError, ClipboardImage, MemoryClipboard};
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
