use aqua_shell::services::{ClipboardError, ClipboardImage};
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) const TEXT: &[&str] = &[
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];
pub(crate) const URI_LIST: &str = "text/uri-list";
pub(crate) const GNOME_FILES: &str = "x-special/gnome-copied-files";
pub(crate) const FILES: &[&str] = &[URI_LIST, GNOME_FILES];
pub(crate) const PNG: &str = "image/png";
pub(crate) const IMAGES: &[&str] = &[PNG];

#[derive(Debug, Clone)]
pub(crate) enum Content {
    Text(String),
    Image(ClipboardImage),
    Files(Vec<PathBuf>),
}

pub(crate) type Bytes = Arc<[u8]>;

#[derive(Debug, Clone)]
pub(crate) struct Encoded {
    pub content: Content,
    pub formats: Vec<(&'static str, Bytes)>,
}

impl Encoded {
    pub(crate) fn new(content: Content) -> Result<Self, ClipboardError> {
        let formats: Vec<(&'static str, Bytes)> = match &content {
            Content::Text(text) => {
                let bytes: Bytes = Arc::from(text.as_bytes());
                TEXT.iter().map(|mime| (*mime, bytes.clone())).collect()
            }
            Content::Image(image) => vec![(PNG, Arc::from(encode_png(image)?))],
            Content::Files(paths) => vec![
                (URI_LIST, Arc::from(uri_list(paths).into_bytes())),
                (GNOME_FILES, Arc::from(gnome_files(paths).into_bytes())),
            ],
        };
        Ok(Self { content, formats })
    }

    pub(crate) fn bytes(&self, mime: &str) -> Option<Bytes> {
        self.formats
            .iter()
            .find(|(offered, _)| *offered == mime)
            .map(|(_, bytes)| bytes.clone())
    }
}

fn file_uris(paths: &[PathBuf]) -> impl Iterator<Item = String> + '_ {
    paths.iter().filter_map(|path| {
        let absolute = match path.is_absolute() {
            true => path.clone(),
            false => std::env::current_dir().ok()?.join(path),
        };
        url::Url::from_file_path(absolute)
            .ok()
            .map(|uri| uri.to_string())
    })
}

pub(crate) fn uri_list(paths: &[PathBuf]) -> String {
    file_uris(paths).map(|uri| uri + "\r\n").collect()
}

pub(crate) fn gnome_files(paths: &[PathBuf]) -> String {
    std::iter::once(String::from("copy"))
        .chain(file_uris(paths))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn parse_files(bytes: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| url::Url::parse(line).ok())
        .filter(|uri| uri.scheme() == "file")
        .filter_map(|uri| uri.to_file_path().ok())
        .collect()
}

pub(crate) fn parse_text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn encode_png(image: &ClipboardImage) -> Result<Vec<u8>, ClipboardError> {
    let width = u32::try_from(image.width()).map_err(|_| ClipboardError::InvalidImage)?;
    let height = u32::try_from(image.height()).map_err(|_| ClipboardError::InvalidImage)?;
    let mut out = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut out),
        image.rgba(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|error| ClipboardError::Os(format!("encoding the clipboard image as PNG: {error}")))?;
    Ok(out)
}

pub(crate) fn decode_png(bytes: &[u8]) -> Result<ClipboardImage, ClipboardError> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| ClipboardError::Os(format!("decoding the clipboard PNG: {error}")))?
        .into_rgba8();
    let (width, height) = decoded.dimensions();
    ClipboardImage::new(width as usize, height as usize, decoded.into_raw())
}
