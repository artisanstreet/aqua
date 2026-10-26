//! `TextRun` mint side (contract §4).
//!
//! The scene vocabulary struct `TextRun { id, revision }` is canonical in
//! `aqua_core::scene` (core owner lands it; `aqua-render` re-exports it).
//! This module does NOT redefine that struct. It mints the runs: a
//! [`RunRegistry`] owns retained [`TextLayout`]s and hands out
//! ([`RunId`], revision) pairs that map 1:1 onto `TextRun { id, revision }`.
//! The renderer resolves a run through [`RunRegistry::resolve`] and shapes
//! no text itself — it renders [`TextLayout::buffer`] with the shared
//! [`FontManager`].
//!
//! Revision bumps on every [`RunRegistry::update`]; the renderer treats
//! (id, revision) as its glyph-cache key and re-resolves only on change.

use std::collections::HashMap;

use crate::layout::TextLayout;

/// Opaque run handle minted here; becomes `TextRun.id` in core's vocabulary.
/// Never 0 (0 is reserved as "no run").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RunId(pub u64);

/// What the renderer gets when resolving a run: the revision to key its
/// caches on, plus the retained layout (shaped lines + buffer).
pub struct RunHandle<'a> {
    pub revision: u64,
    pub layout: &'a TextLayout,
}

struct RunEntry {
    revision: u64,
    layout: TextLayout,
}

/// Owns retained layouts and mints/resolves run handles.
///
/// Memory: one retained shaping `Buffer` per live run. Eviction is the
/// UI's job — call [`RunRegistry::remove`] when the text node unmounts.
/// Unbounded growth without removal is a UI bug, not a cache policy.
pub struct RunRegistry {
    next: u64,
    runs: HashMap<u64, RunEntry>,
}

impl Default for RunRegistry {
    /// Identical to [`RunRegistry::new`]: first minted id is `RunId(1)`,
    /// never the reserved `RunId(0)`. (`#[derive(Default)]` would start
    /// at 0 and mint the invalid id — deliberately not derived.)
    fn default() -> Self {
        Self::new()
    }
}

impl RunRegistry {
    pub fn new() -> Self {
        Self {
            next: 1,
            runs: HashMap::new(),
        }
    }

    /// Retain `layout` and mint a fresh run id at revision 0.
    pub fn insert(&mut self, layout: TextLayout) -> (RunId, u64) {
        let id = self.next;
        self.next += 1;
        self.runs.insert(
            id,
            RunEntry {
                revision: 0,
                layout,
            },
        );
        (RunId(id), 0)
    }

    /// Replace the layout for `id`, bumping its revision. Returns the new
    /// revision, or `None` for an unknown id (stale handle: fail closed).
    pub fn update(&mut self, id: RunId, layout: TextLayout) -> Option<u64> {
        let entry = self.runs.get_mut(&id.0)?;
        entry.revision += 1;
        entry.layout = layout;
        Some(entry.revision)
    }

    /// Forget a run (node unmounted). Returns false if unknown.
    pub fn remove(&mut self, id: RunId) -> bool {
        self.runs.remove(&id.0).is_some()
    }

    pub fn len(&self) -> usize {
        self.runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Resolve a run for rendering. `None` = stale/unknown id.
    pub fn resolve(&self, id: RunId) -> Option<RunHandle<'_>> {
        self.runs.get(&id.0).map(|e| RunHandle {
            revision: e.revision,
            layout: &e.layout,
        })
    }

    /// Resolve a run against the revision the scene recorded.
    ///
    /// * `Ok` — id known and revision current.
    /// * `Err(ResolveError::RevisionMismatch { current })` — id known but
    ///   the scene holds a stale revision (text updated after the scene
    ///   commit; render the current layout and count the mismatch).
    /// * `Err(ResolveError::Unknown)` — id was never minted or was removed.
    ///   This is an explicit error, never a debug fallback string.
    pub fn resolve_expected(
        &self,
        id: RunId,
        revision: u64,
    ) -> Result<RunHandle<'_>, ResolveError> {
        match self.runs.get(&id.0) {
            None => Err(ResolveError::Unknown),
            Some(e) if e.revision != revision => Err(ResolveError::RevisionMismatch {
                current: e.revision,
            }),
            Some(e) => Ok(RunHandle {
                revision: e.revision,
                layout: &e.layout,
            }),
        }
    }
}

/// Explicit failure to resolve a run handle. Unknown ids are errors, not
/// an excuse to render placeholder text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// Never minted or already removed.
    Unknown,
    /// Scene holds an older revision; `current` is the live one.
    RevisionMismatch { current: u64 },
}

/// Reference-counted shared ownership of one [`RunRegistry`].
///
/// Clone the handle wherever text is minted (UI) or resolved (renderer);
/// lock it for each operation. `Send + Sync` (asserted by test).
#[derive(Clone, Default)]
pub struct SharedRuns {
    inner: std::sync::Arc<std::sync::Mutex<RunRegistry>>,
}

impl SharedRuns {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(RunRegistry::new())),
        }
    }

    /// Lock the shared registry. Guard holds `&mut RunRegistry`.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, RunRegistry> {
        self.inner.lock().expect("run registry lock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::TextStyle;

    fn laid_out(text: &str) -> TextLayout {
        let mut t = TextLayout::new();
        t.set_text(text, &TextStyle::default());
        t
    }

    #[test]
    fn mint_update_remove() {
        let mut reg = RunRegistry::new();
        let (id, rev) = reg.insert(laid_out("a"));
        assert_eq!(rev, 0);
        assert_ne!(id, RunId(0));
        assert_eq!(reg.resolve(id).unwrap().revision, 0);

        let rev1 = reg.update(id, laid_out("ab")).unwrap();
        assert_eq!(rev1, 1);
        assert_eq!(reg.resolve(id).unwrap().revision, 1);

        assert!(reg.remove(id));
        assert!(reg.resolve(id).is_none());
        assert!(reg.update(id, laid_out("x")).is_none());
        assert!(!reg.remove(id));
    }

    #[test]
    fn default_path_matches_new() {
        // Constructors and Default must not differ in validity: Default
        // reserves RunId(0) exactly like new() (first mint is RunId(1)).
        let mut via_new = RunRegistry::new();
        let mut via_default = RunRegistry::default();
        assert_eq!(via_new.insert(laid_out("a")).0, RunId(1));
        assert_eq!(via_default.insert(laid_out("a")).0, RunId(1));
        let shared_new = SharedRuns::new();
        let shared_default = SharedRuns::default();
        assert_eq!(shared_new.lock().insert(laid_out("a")).0, RunId(1));
        assert_eq!(shared_default.lock().insert(laid_out("a")).0, RunId(1));
    }

    #[test]
    fn ids_are_unique() {
        let mut reg = RunRegistry::new();
        let (a, _) = reg.insert(laid_out("a"));
        let (b, _) = reg.insert(laid_out("b"));
        assert_ne!(a, b);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn expected_revision_distinguishes_stale_and_unknown() {
        let mut reg = RunRegistry::new();
        let (id, rev0) = reg.insert(laid_out("a"));
        assert!(reg.resolve_expected(id, rev0).is_ok());

        let rev1 = reg.update(id, laid_out("ab")).unwrap();
        // Old scene revision: explicit mismatch carrying the live revision.
        match reg.resolve_expected(id, rev0) {
            Err(ResolveError::RevisionMismatch { current }) => assert_eq!(current, rev1),
            other => panic!("expected RevisionMismatch, got {}", other.is_ok()),
        }
        assert!(reg.resolve_expected(id, rev1).is_ok());

        // Unknown id: explicit error.
        assert!(matches!(
            reg.resolve_expected(RunId(9999), 0),
            Err(ResolveError::Unknown)
        ));

        // Removed id becomes unknown, not silently resolvable.
        assert!(reg.remove(id));
        assert!(matches!(
            reg.resolve_expected(id, rev1),
            Err(ResolveError::Unknown)
        ));
    }

    #[test]
    fn shared_runs_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SharedRuns>();
        assert_send_sync::<RunRegistry>();
    }
}
