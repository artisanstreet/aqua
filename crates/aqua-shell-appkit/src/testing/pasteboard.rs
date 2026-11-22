use crate::services::PasteboardName;
use objc2_app_kit::{NSBitmapImageRep, NSPasteboard, NSPasteboardTypeTIFF};
use objc2_foundation::{NSData, NSString};

fn board(name: &PasteboardName) -> objc2::rc::Retained<NSPasteboard> {
    let PasteboardName::Named(name) = name else {
        panic!("invariant: tests write to a named pasteboard, never the user's");
    };
    NSPasteboard::pasteboardWithName(&NSString::from_str(name))
}

pub fn write_tiff_only(name: &PasteboardName, png: &[u8]) {
    let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(png))
        .expect("the test PNG is a bitmap");
    let tiff = rep.TIFFRepresentation().expect("a bitmap has a TIFF form");
    let board = board(name);
    board.clearContents();
    assert!(
        board.setData_forType(Some(&tiff), unsafe { NSPasteboardTypeTIFF }),
        "the pasteboard takes the TIFF"
    );
}
