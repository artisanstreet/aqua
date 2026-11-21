use aqua_shell::services::{Clipboard, ClipboardError, ClipboardImage};
use image::ImageEncoder;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2::ClassType;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardURLReadingFileURLsOnlyKey,
    NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString, NSURL};
use std::path::{Path, PathBuf};

pub const PASTEBOARD_VARIABLE: &str = "AQUA_MAC_PASTEBOARD";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteboardName {
    General,
    Named(String),
}

impl PasteboardName {
    pub fn from_environment() -> PasteboardName {
        match std::env::var(PASTEBOARD_VARIABLE) {
            Ok(name) if !name.is_empty() => PasteboardName::Named(name),
            _ => PasteboardName::General,
        }
    }
}

fn open(name: &PasteboardName) -> Retained<NSPasteboard> {
    match name {
        PasteboardName::General => NSPasteboard::generalPasteboard(),
        PasteboardName::Named(name) => NSPasteboard::pasteboardWithName(&NSString::from_str(name)),
    }
}

pub(crate) fn file_paths(board: &NSPasteboard) -> Vec<PathBuf> {
    let classes = NSArray::<AnyClass>::from_slice(&[NSURL::class()]);
    let yes = NSNumber::new_bool(true);
    let options = NSDictionary::<_, AnyObject>::from_slices(
        &[unsafe { NSPasteboardURLReadingFileURLsOnlyKey }],
        &[&*yes as &AnyObject],
    );
    let objects = unsafe { board.readObjectsForClasses_options(&classes, Some(&options)) };
    match objects {
        None => Vec::new(),
        Some(objects) => objects
            .iter()
            .filter_map(|object| {
                object
                    .downcast_ref::<NSURL>()
                    .and_then(|url| url.path())
                    .map(|path| PathBuf::from(path.to_string()))
            })
            .collect(),
    }
}

fn os(step: &str) -> ClipboardError {
    ClipboardError::Os(format!("NSPasteboard refused to {step}"))
}

fn decode_png(bytes: &[u8]) -> Result<ClipboardImage, ClipboardError> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| {
            ClipboardError::Os(format!("the pasteboard PNG does not decode: {error}"))
        })?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    ClipboardImage::new(width as usize, height as usize, decoded.into_raw())
}

fn encode_png(image: &ClipboardImage) -> Result<Vec<u8>, ClipboardError> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            image.rgba(),
            image.width() as u32,
            image.height() as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| {
            ClipboardError::Os(format!("the image does not encode as PNG: {error}"))
        })?;
    Ok(bytes)
}

fn tiff_as_png(tiff: &NSData) -> Result<Vec<u8>, ClipboardError> {
    let rep = NSBitmapImageRep::imageRepWithData(tiff).ok_or(ClipboardError::Os(
        "the pasteboard TIFF is not a bitmap".into(),
    ))?;
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or(ClipboardError::Os(
        "the pasteboard TIFF does not convert to PNG".into(),
    ))?;
    Ok(png.to_vec())
}

pub struct SystemClipboard {
    board: Retained<NSPasteboard>,
}

impl SystemClipboard {
    pub fn open(name: &PasteboardName) -> SystemClipboard {
        SystemClipboard { board: open(name) }
    }

    fn replace(&self) {
        self.board.clearContents();
    }
}

impl Clipboard for SystemClipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Ok(self
            .board
            .stringForType(unsafe { NSPasteboardTypeString })
            .map(|text| text.to_string()))
    }

    fn set_text(&mut self, text: String) -> Result<(), ClipboardError> {
        self.replace();
        self.board
            .setString_forType(&NSString::from_str(&text), unsafe {
                NSPasteboardTypeString
            })
            .then_some(())
            .ok_or_else(|| os("take the text"))
    }

    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.replace();
        Ok(())
    }

    fn get_image(&mut self) -> Result<Option<ClipboardImage>, ClipboardError> {
        let png = unsafe { NSPasteboardTypePNG };
        let tiff = unsafe { NSPasteboardTypeTIFF };
        let offered = self
            .board
            .availableTypeFromArray(&NSArray::from_slice(&[png, tiff]));
        let bytes = match offered {
            None => return Ok(None),
            Some(kind) if &*kind == png => self.board.dataForType(png).map(|data| data.to_vec()),
            Some(_) => self
                .board
                .dataForType(tiff)
                .map(|data| tiff_as_png(&data))
                .transpose()?,
        };
        bytes.map(|bytes| decode_png(&bytes)).transpose()
    }

    fn set_image(&mut self, image: ClipboardImage) -> Result<(), ClipboardError> {
        let png = encode_png(&image)?;
        self.replace();
        self.board
            .setData_forType(Some(&NSData::with_bytes(&png)), unsafe {
                NSPasteboardTypePNG
            })
            .then_some(())
            .ok_or_else(|| os("take the PNG"))
    }

    fn get_files(&mut self) -> Result<Option<Vec<PathBuf>>, ClipboardError> {
        let paths = file_paths(&self.board);
        Ok((!paths.is_empty()).then_some(paths))
    }

    fn set_files(&mut self, files: Vec<PathBuf>) -> Result<(), ClipboardError> {
        self.replace();
        if files.is_empty() {
            return Ok(());
        }
        let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = files
            .iter()
            .map(|path| ProtocolObject::from_retained(file_url(path)))
            .collect();
        self.board
            .writeObjects(&NSArray::from_retained_slice(&urls))
            .then_some(())
            .ok_or_else(|| os("take the file URLs"))
    }
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}
