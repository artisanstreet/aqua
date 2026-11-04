//! Shared font discovery and fallback, wrapping the shaping engine.
//!
//! `FontManager` owns one `cosmic_text::FontSystem`: system fonts are loaded
//! once, custom fonts can be added from bytes or files, and every layout in
//! the process reuses it. The shaping engine performs script-aware fallback
//! (Latin, CJK, Arabic, emoji, …) internally; this module only exposes
//! discovery queries and explicit font loading on top.

/// Shared font database + shaping-engine state.
///
/// One `FontManager` per UI process is the expected shape: every layout in
/// the process reuses its font database and shaping caches. Ownership is
/// shared in practice through [`SharedFonts`] (`Arc<Mutex<…>>`), which is
/// `Send + Sync` (verified by test) so the UI thread and render code hand
/// the same instance back and forth — never two divergent databases.
///
/// IMPORTANT for `aqua-render`: glyphon's `TextRenderer::prepare` takes
/// `&mut cosmic_text::FontSystem` from the *caller*, so pass
/// `fonts.lock().font_system_mut()` together with the retained
/// [`crate::TextLayout::buffer`] and measurement and pixels share one
/// shaping. Do not create a second `FontSystem` for rendering.
pub struct FontManager {
    system: cosmic_text::FontSystem,
    /// Monotonic counter bumped by every [`FontManager::load_font_bytes`] /
    /// [`FontManager::load_font_file`]. Shaping caches key on this instead
    /// of face count: replacing a font with another of the same face count
    /// still invalidates (face-count proxy would miss it).
    revision: u64,
    formation_cache: std::collections::HashMap<(String, String), bool>,
}

impl FontManager {
    /// Create a manager with all system fonts loaded (fontconfig on Linux,
    /// CoreText/DirectWrite backed equivalents elsewhere via fontdb).
    pub fn new() -> Self {
        Self {
            system: cosmic_text::FontSystem::new(),
            revision: 0,
            formation_cache: std::collections::HashMap::new(),
        }
    }

    /// Create a manager with no system fonts. Useful for hermetic tests and
    /// for hosts that supply their own font set via [`FontManager::load_font_bytes`].
    ///
    /// Note: the shaping engine loads system fonts inside `new_with_fonts`
    /// even for an empty iterator, so this builds from an empty database
    /// with a fixed `"und"` (undetermined) locale instead.
    pub fn new_without_system_fonts() -> Self {
        Self {
            system: cosmic_text::FontSystem::new_with_locale_and_db(
                "und".to_string(),
                fontdb::Database::new(),
            ),
            revision: 0,
            formation_cache: std::collections::HashMap::new(),
        }
    }

    /// Monotonic font-generation counter. Layout caches key on this; any
    /// font load invalidates shaping. (Face count alone would miss a
    /// same-count font replacement.)
    pub fn font_revision(&self) -> u64 {
        self.revision
    }

    /// Load a font from in-memory data (TTF/OTF/WOFF… whatever fontdb parses).
    /// The data is reference-counted internally; the caller need not retain it.
    /// Bumps [`FontManager::font_revision`].
    pub fn load_font_bytes(&mut self, data: Vec<u8>) {
        self.system.db_mut().load_font_data(data);
        self.revision = self.revision.wrapping_add(1);
        self.formation_cache.clear();
    }

    /// Load a font from a file path. Bumps [`FontManager::font_revision`]
    /// on success.
    pub fn load_font_file(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), std::io::Error> {
        let r = self.system.db_mut().load_font_file(path);
        if r.is_ok() {
            self.revision = self.revision.wrapping_add(1);
            self.formation_cache.clear();
        }
        r
    }

    /// Number of font faces currently in the database.
    pub fn face_count(&self) -> usize {
        self.system.db().faces().count()
    }

    /// True if at least one face matches the given family name.
    pub fn has_family(&self, family: &str) -> bool {
        self.system.db().faces().any(|f| {
            f.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
        })
    }

    /// Names of all families in the database, sorted and deduplicated.
    pub fn family_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .system
            .db()
            .faces()
            .flat_map(|f| f.families.iter().map(|(name, _)| name.clone()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Copy out the raw font data backing `face`, e.g. to load an
    /// identical copy into another font system. Prefer sharing one
    /// `FontManager` (see [`SharedFonts`]) over duplicating databases;
    /// this exists for hosts that must feed a foreign font API.
    pub fn export_face_data(&self, face: fontdb::ID) -> Option<Vec<u8>> {
        self.system
            .db()
            .with_face_data(face, |data, _| data.to_vec())
    }

    pub(crate) fn fallback_spans(
        &mut self,
        text: &str,
        style: &crate::style::TextStyle,
    ) -> Vec<(std::ops::Range<usize>, Option<String>)> {
        use unicode_segmentation::UnicodeSegmentation;
        if text.is_empty() {
            return vec![(0..0, None)];
        }
        if text.is_ascii() {
            return vec![(0..text.len(), None)];
        }
        let family = style
            .families
            .first()
            .map(crate::style::FontFamily::to_cosmic)
            .unwrap_or(cosmic_text::Family::SansSerif);
        let query = fontdb::Query {
            families: &[family],
            weight: style.weight.to_cosmic(),
            stretch: style.stretch.to_cosmic(),
            style: style.slant.to_cosmic(),
        };
        let Some(primary_id) = self.system.db().query(&query) else {
            return vec![(0..text.len(), None)];
        };
        let Some(primary) = self.system.get_font(primary_id, style.weight.to_cosmic()) else {
            return vec![(0..text.len(), None)];
        };
        let names: Vec<String> = if style.families.len() > 1 {
            style
                .families
                .iter()
                .skip(1)
                .filter_map(|family| match family {
                    crate::style::FontFamily::Named(name) => Some(name.clone()),
                    _ => None,
                })
                .collect()
        } else {
            [
                "Segoe UI Emoji",
                "Noto Color Emoji",
                "Apple Color Emoji",
                "Twemoji Mozilla",
            ]
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
        };
        let mut candidates = Vec::new();
        for name in names {
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(&name)],
                weight: style.weight.to_cosmic(),
                stretch: style.stretch.to_cosmic(),
                style: style.slant.to_cosmic(),
            };
            let Some(id) = self.system.db().query(&query) else {
                continue;
            };
            let Some(face) = self.system.get_font(id, style.weight.to_cosmic()) else {
                continue;
            };
            let known_emoji = self.system.db().face(id).is_some_and(|face| {
                matches!(
                    face.post_script_name.as_str(),
                    "SegoeUIEmoji"
                        | "NotoColorEmoji"
                        | "AppleColorEmoji"
                        | ".AppleColorEmojiUI"
                        | "TwemojiMozilla"
                )
            });
            candidates.push((name, id, face, known_emoji));
        }
        if candidates.is_empty() {
            return vec![(0..text.len(), None)];
        }
        let mut spans = Vec::new();
        let mut start = 0;
        let mut slot: Option<usize> = None;
        for (index, grapheme) in text.grapheme_indices(true) {
            let next = select_cluster(
                grapheme,
                candidates.len(),
                |ch| primary.as_swash().charmap().map(ch) != 0,
                |slot, ch| candidates[slot].2.as_swash().charmap().map(ch) != 0,
                |slot| {
                    let (name, id, _, emoji) = &candidates[slot];
                    let key = (name.clone(), grapheme.to_owned());
                    if let Some(value) = self.formation_cache.get(&key) {
                        return *value;
                    }
                    let metrics = cosmic_text::Metrics::new(
                        style.size_px.max(1.0),
                        style.line_height_px.max(1.0),
                    );
                    let mut probe = cosmic_text::Buffer::new(&mut self.system, metrics);
                    let attrs = cosmic_text::Attrs::new().family(cosmic_text::Family::Name(name));
                    probe.set_text(grapheme, &attrs, cosmic_text::Shaping::Advanced, None);
                    probe.shape_until_scroll(&mut self.system, false);
                    let mut count = 0usize;
                    let mut valid = true;
                    for glyph in probe.layout_runs().flat_map(|line| line.glyphs) {
                        if glyph.glyph_id == 0 || glyph.font_id != *id {
                            valid = false;
                            break;
                        }
                        if *emoji && glyph.glyph_id == 3 {
                            continue;
                        }
                        count += 1;
                    }
                    let result = valid && count == 1;
                    if self.formation_cache.len() < 512 {
                        self.formation_cache.insert(key, result);
                    }
                    result
                },
            );
            if next != slot {
                if index > start {
                    spans.push((start..index, slot.map(|slot| candidates[slot].0.clone())));
                }
                start = index;
                slot = next;
            }
        }
        spans.push((
            start..text.len(),
            slot.map(|slot| candidates[slot].0.clone()),
        ));
        spans
    }

    /// Access the shaping engine's font system for layout calls.
    pub fn font_system(&self) -> &cosmic_text::FontSystem {
        &self.system
    }

    /// Mutable access for layout calls (shaping requires `&mut`).
    ///
    /// Do NOT load fonts through the returned handle (`db_mut()` etc.):
    /// that bypasses [`FontManager::font_revision`] and shaping caches
    /// would keep stale glyphs. Use [`FontManager::load_font_bytes`] /
    /// [`FontManager::load_font_file`] instead.
    pub fn font_system_mut(&mut self) -> &mut cosmic_text::FontSystem {
        &mut self.system
    }
}

/// Reference-counted shared ownership of one [`FontManager`].
///
/// Clone the handle (cheap) wherever text is measured, edited or rendered;
/// lock it for the duration of each operation. Hold the guard across a
/// whole layout-or-render call but never across frames.
#[derive(Clone, Default)]
pub struct SharedFonts {
    inner: std::sync::Arc<std::sync::Mutex<FontManager>>,
}

impl SharedFonts {
    /// System fonts loaded.
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(FontManager::new())),
        }
    }

    /// No system fonts; the host adds its own via [`FontManager`].
    pub fn without_system_fonts() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(
                FontManager::new_without_system_fonts(),
            )),
        }
    }

    /// Lock the shared manager. Guard holds `&mut FontManager`.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, FontManager> {
        self.inner.lock().expect("font manager lock poisoned")
    }
}

impl Default for FontManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn select_cluster(
    grapheme: &str,
    candidate_count: usize,
    primary_covers: impl Fn(char) -> bool,
    candidate_covers: impl Fn(usize, char) -> bool,
    mut forms_single: impl FnMut(usize) -> bool,
) -> Option<usize> {
    if grapheme.is_ascii() || grapheme.chars().all(&primary_covers) {
        return None;
    }
    let full: Vec<_> = (0..candidate_count)
        .filter(|index| grapheme.chars().all(|ch| candidate_covers(*index, ch)))
        .collect();
    if let Some(&first) = full.first() {
        if grapheme.chars().count() == 1 {
            return Some(first);
        }
        return full
            .into_iter()
            .find(|index| forms_single(*index))
            .or(Some(first));
    }
    let first = grapheme.chars().next()?;
    if primary_covers(first) {
        None
    } else {
        (0..candidate_count).find(|index| candidate_covers(*index, first))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_fonts_load() {
        let mgr = FontManager::new();
        assert!(mgr.face_count() > 0, "expected system fonts to load");
    }

    #[test]
    fn empty_manager_has_no_faces() {
        let mgr = FontManager::new_without_system_fonts();
        assert_eq!(mgr.face_count(), 0);
    }

    #[test]
    fn shared_fonts_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SharedFonts>();
        assert_send_sync::<FontManager>();
    }

    #[test]
    fn default_path_matches_new() {
        let via_new = FontManager::new();
        let via_default = FontManager::default();
        assert_eq!(via_new.face_count(), via_default.face_count());
        assert_eq!(via_new.font_revision(), via_default.font_revision());
        let shared_new = SharedFonts::new();
        let shared_default = SharedFonts::default();
        assert_eq!(
            shared_new.lock().face_count(),
            shared_default.lock().face_count()
        );
    }

    #[test]
    fn font_loads_bump_revision() {
        let mut mgr = FontManager::new_without_system_fonts();
        assert_eq!(mgr.font_revision(), 0);
        let before = mgr.face_count();
        let r0 = mgr.font_revision();
        mgr.load_font_file("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
            .expect("test host needs DejaVuSans");
        assert_eq!(mgr.font_revision(), r0.wrapping_add(1));
        assert!(mgr.face_count() >= before);
        let data = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
            .expect("test host needs DejaVuSans");
        mgr.load_font_bytes(data);
        assert_eq!(mgr.font_revision(), r0.wrapping_add(2));
    }
}
