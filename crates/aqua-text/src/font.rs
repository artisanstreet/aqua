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
}

impl FontManager {
    /// Create a manager with all system fonts loaded (fontconfig on Linux,
    /// CoreText/DirectWrite backed equivalents elsewhere via fontdb).
    pub fn new() -> Self {
        Self {
            system: cosmic_text::FontSystem::new(),
            revision: 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_fonts_load() {
        let mgr = FontManager::new();
        // The CI/dev host ships DejaVu at minimum; assert non-empty rather
        // than a specific family so the test stays portable.
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
        // Constructors and Default must not differ in validity:
        // Default loads system fonts exactly like new().
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
        // Reload an already-present system font file: face count may be
        // unchanged on some hosts, but the revision must still advance so
        // same-count replacement invalidates shaping caches.
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
