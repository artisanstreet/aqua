use std::path::PathBuf;
use std::thread::JoinHandle;

use aqua_fonts_system::{default_fallback_index_path, read_cache_file, write_cache_file};
use aqua_text::style::{FontFamily, FontWeight};
use aqua_text::{SharedFonts, TextStyle};

fn cache_root() -> Option<PathBuf> {
    default_fallback_index_path().and_then(|path| path.parent().map(PathBuf::from))
}

#[must_use]
pub fn pipeline_cache_path() -> Option<PathBuf> {
    cache_root().map(|root| root.join("pipelines.cache"))
}

#[must_use]
pub fn load_pipeline_cache() -> Option<Vec<u8>> {
    pipeline_cache_path()
        .and_then(read_cache_file)
        .filter(|bytes| !bytes.is_empty())
}

#[derive(Debug, Default)]
pub struct PipelineCacheSaver {
    written: Option<u64>,
}

fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |state, byte| {
        (state ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    }) ^ bytes.len() as u64
}

impl PipelineCacheSaver {
    pub fn save(&mut self, serialized: Option<Vec<u8>>) -> bool {
        let Some(bytes) = serialized.filter(|bytes| !bytes.is_empty()) else {
            return false;
        };
        let print = fingerprint(&bytes);
        match (self.written == Some(print), pipeline_cache_path()) {
            (false, Some(path)) => {
                let saved = write_cache_file(path, &bytes).is_ok();
                if saved {
                    self.written = Some(print);
                }
                saved
            }
            _ => false,
        }
    }
}

pub fn attach_fallback_index(fonts: &SharedFonts) {
    let persisted = default_fallback_index_path().and_then(read_cache_file);
    fonts.lock().use_fallback_index(persisted.as_deref());
}

pub fn save_fallback_index(fonts: &SharedFonts) -> bool {
    let snapshot = fonts.lock().fallback_index_snapshot();
    match (snapshot, default_fallback_index_path()) {
        (Some(bytes), Some(path)) => write_cache_file(path, &bytes).is_ok(),
        _ => false,
    }
}

#[must_use]
pub fn prewarm_styles() -> Vec<TextStyle> {
    [
        (FontFamily::SansSerif, FontWeight::NORMAL),
        (FontFamily::SansSerif, FontWeight::MEDIUM),
        (FontFamily::SansSerif, FontWeight(600)),
        (FontFamily::SansSerif, FontWeight::BOLD),
        (FontFamily::Monospace, FontWeight::NORMAL),
        (FontFamily::Monospace, FontWeight::BOLD),
    ]
    .into_iter()
    .map(|(family, weight)| TextStyle {
        families: vec![family],
        weight,
        ..TextStyle::default()
    })
    .collect()
}

pub fn spawn_prewarm(fonts: SharedFonts, styles: Vec<TextStyle>) -> Option<JoinHandle<usize>> {
    std::thread::Builder::new()
        .name("aqua-prewarm".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let jobs = fonts.lock().prewarm_jobs(&styles);
            let queued = jobs.len();
            let installed = jobs
                .into_iter()
                .filter_map(aqua_text::PrewarmJob::build)
                .filter(|entry| fonts.lock().install_prewarmed(entry.clone()))
                .count();
            save_fallback_index(&fonts);
            if crate::profile::enabled() {
                eprintln!(
                    "aqua-startup prewarm queued={queued} installed={installed} ms={:.1}",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
            installed
        })
        .ok()
}

pub fn start_text_services(fonts: &SharedFonts) -> Option<JoinHandle<usize>> {
    attach_fallback_index(fonts);
    spawn_prewarm(fonts.clone(), prewarm_styles())
}
