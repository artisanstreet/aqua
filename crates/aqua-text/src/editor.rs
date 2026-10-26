//! The editing model: text + selection + IME composition + undo/redo.
//!
//! `TextEditor` owns a [`TextModel`] and a [`Selection`] and implements
//! grapheme-safe movement/deletion, word jumps, selection extension,
//! single-line policy, IME preedit commit/cancel, and bounded undo/redo.
//! It knows nothing about pixels: pair it with [`crate::TextLayout`] (same
//! source coordinates) for caret rectangles and hit-testing.
//!
//! Byte indices throughout. The editor keeps the selection on grapheme
//! boundaries after every operation.

use crate::model::TextModel;
use crate::selection::Selection;
use crate::style::LineMode;

/// One undoable step.
#[derive(Clone, Debug)]
struct EditRecord {
    /// Byte position where the change starts (in pre-edit coordinates for
    /// deletes, post-edit for inserts — resolved at apply time).
    kind: EditKind,
    before: Selection,
    after: Selection,
}

#[derive(Clone, Debug)]
enum EditKind {
    Insert {
        pos: usize,
        text: String,
    },
    Delete {
        pos: usize,
        text: String,
    },
    /// One user operation that replaced `deleted` with `inserted` at `pos`
    /// (typing over a selection, IME commit over a selection). Undo and redo
    /// each restore content AND selection in a single step.
    Replace {
        pos: usize,
        deleted: String,
        inserted: String,
    },
}

/// Text removed by starting a preedit over a selection. Uncommitted, so it
/// lives outside undo history — but cancel must restore it losslessly and
/// commit/keystrokes must account for it (see below).
#[derive(Clone, Debug)]
struct Replaced {
    pos: usize,
    deleted: String,
    before: Selection,
}

/// Composition (IME preedit) state.
#[derive(Clone, Debug)]
struct Composition {
    /// Byte index in committed text where the preedit sits.
    start: usize,
    text: String,
    /// Cursor within the preedit, as a byte range (may be empty).
    cursor: std::ops::Range<usize>,
    /// Selection content removed to make room for the preedit, if any.
    replaced: Option<Replaced>,
}

/// Editable text with selection, IME and undo.
pub struct TextEditor {
    model: TextModel,
    selection: Selection,
    line_mode: LineMode,
    ime: Option<Composition>,
    undo_stack: Vec<EditRecord>,
    redo_stack: Vec<EditRecord>,
    undo_limit: usize,
}

impl TextEditor {
    pub fn new(line_mode: LineMode) -> Self {
        Self {
            model: TextModel::new(String::new()),
            selection: Selection::default(),
            line_mode,
            ime: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_limit: 100,
        }
    }

    pub fn with_text(text: &str, line_mode: LineMode) -> Self {
        let mut ed = Self::new(line_mode);
        ed.set_text(text);
        ed
    }

    pub fn text(&self) -> &str {
        self.model.text()
    }

    /// Committed text plus the active preedit spliced in (what the user
    /// sees). Pass this to `TextLayout`; coordinates differ from committed
    /// text while composing — see [`TextEditor::composition_range`].
    pub fn display_text(&self) -> String {
        match &self.ime {
            Some(c) => {
                let mut s = self.model.text().to_string();
                s.insert_str(c.start.min(s.len()), &c.text);
                s
            }
            None => self.model.text().to_string(),
        }
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// Place the caret at byte `pos`, collapsing any selection. The position
    /// is clamped into the text and snapped to the nearest grapheme boundary
    /// (ties snap to the previous boundary), so UI hit-testing can pass raw
    /// coordinates safely. Cancels any active preedit (displaced text is
    /// restored losslessly) — same policy as keyboard caret movement.
    /// Never touches undo history.
    pub fn set_caret(&mut self, pos: usize) {
        self.set_selection(Selection::caret_at(pos));
    }

    /// Replace the selection wholesale. Both ends are clamped into the text
    /// and snapped to the nearest grapheme boundary (ties snap down);
    /// anchor/caret order is preserved exactly as given, so shift-extend
    /// state round-trips through the caller. Cancels any active preedit
    /// (displaced text restored losslessly). Never touches undo history.
    pub fn set_selection(&mut self, selection: Selection) {
        self.cancel_composition();
        let len = self.model.len_bytes();
        let anchor = snap_nearest(&self.model, selection.anchor.min(len));
        let caret = snap_nearest(&self.model, selection.caret.min(len));
        self.selection = Selection::new(anchor, caret);
    }

    pub fn line_mode(&self) -> LineMode {
        self.line_mode
    }

    pub fn set_line_mode(&mut self, mode: LineMode) {
        self.line_mode = mode;
        if mode == LineMode::Single {
            self.cancel_composition();
            if self.model.text().contains('\n') {
                let clean: String = self.model.text().replace('\n', " ");
                self.model.set_text(clean);
                self.selection.clamp(self.model.len_bytes());
                // Folding changed byte positions: every stored record is
                // stale, and replaying one could restore a newline that is
                // now illegal. Drop history instead of corrupting text.
                self.undo_stack.clear();
                self.redo_stack.clear();
            }
        }
    }

    pub fn revision(&self) -> u64 {
        self.model.revision()
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    /// Maximum undo steps retained. Older steps are dropped from the front.
    pub fn set_undo_limit(&mut self, limit: usize) {
        self.undo_limit = limit.max(1);
        while self.undo_stack.len() > self.undo_limit {
            self.undo_stack.remove(0);
        }
    }

    /// Replace all text, clearing selection, IME and undo history.
    /// Use for external/model-driven changes, not for keystrokes.
    pub fn set_text(&mut self, text: &str) {
        let text = self.apply_line_policy(text);
        self.model.set_text(text);
        self.selection.collapse_to(0);
        self.selection.clamp(self.model.len_bytes());
        self.ime = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Selected text in committed coordinates, or `None` for a bare caret.
    pub fn selected_text(&self) -> Option<&str> {
        if self.selection.is_caret() {
            return None;
        }
        let r = self.selection.range();
        self.model.text().get(r)
    }

    // ---- input operations (each is one undo step) ----

    /// Type or paste `s`, replacing the selection. Newlines are stripped in
    /// single-line mode. Returns the new caret position.
    pub fn insert(&mut self, s: &str) -> usize {
        self.commit_composition_if_any_without_record();
        let s = self.apply_line_policy(s);
        let before = self.selection;
        let range = self.selection.range();
        let deleted = if range.is_empty() {
            String::new()
        } else {
            self.model.delete(range.clone())
        };
        let ins = self.model.insert(range.start, &s);
        let caret = ins.end;
        self.selection.collapse_to(caret);
        // A replace is ONE user operation: a single Replace record restores
        // both content and selection in one undo. Plain typing still merges
        // consecutive single chars for natural Ctrl+Z granularity.
        if !deleted.is_empty() {
            if s.is_empty() {
                self.push_record(EditRecord {
                    kind: EditKind::Delete {
                        pos: range.start,
                        text: deleted,
                    },
                    before,
                    after: Selection::caret_at(range.start),
                });
            } else {
                self.push_record(EditRecord {
                    kind: EditKind::Replace {
                        pos: range.start,
                        deleted,
                        inserted: s,
                    },
                    before,
                    after: Selection::caret_at(caret),
                });
            }
        } else if !(self.try_merge_insert(range.start, &s, before, caret)) {
            self.push_record(EditRecord {
                kind: EditKind::Insert {
                    pos: range.start,
                    text: s,
                },
                before,
                after: Selection::caret_at(caret),
            });
        }
        self.redo_stack.clear();
        caret
    }

    /// Delete selection, or the grapheme before the caret.
    pub fn delete_backward(&mut self) -> bool {
        if !self.selection.is_caret() {
            self.delete_selection();
            return true;
        }
        self.commit_composition_if_any_without_record();
        let caret = self.selection.caret;
        let prev = self.model.prev_grapheme(caret);
        if prev == caret {
            return false;
        }
        let before = self.selection;
        let removed = self.model.delete(prev..caret);
        self.selection.collapse_to(prev);
        self.push_record(EditRecord {
            kind: EditKind::Delete {
                pos: prev,
                text: removed,
            },
            before,
            after: self.selection,
        });
        self.redo_stack.clear();
        true
    }

    /// Delete selection, or the grapheme after the caret.
    pub fn delete_forward(&mut self) -> bool {
        if !self.selection.is_caret() {
            self.delete_selection();
            return true;
        }
        self.commit_composition_if_any_without_record();
        let caret = self.selection.caret;
        let next = self.model.next_grapheme(caret);
        if next == caret {
            return false;
        }
        let before = self.selection;
        let removed = self.model.delete(caret..next);
        self.selection.collapse_to(caret);
        self.push_record(EditRecord {
            kind: EditKind::Delete {
                pos: caret,
                text: removed,
            },
            before,
            after: self.selection,
        });
        self.redo_stack.clear();
        true
    }

    /// Delete the current selection. No-op for a bare caret.
    pub fn delete_selection(&mut self) {
        let range = self.selection.range();
        if range.is_empty() {
            return;
        }
        self.commit_composition_if_any_without_record();
        let before = self.selection;
        let removed = self.model.delete(range.clone());
        self.selection.collapse_to(range.start);
        self.push_record(EditRecord {
            kind: EditKind::Delete {
                pos: range.start,
                text: removed,
            },
            before,
            after: self.selection,
        });
        self.redo_stack.clear();
    }

    // ---- movement (not undoable) ----

    fn move_caret(&mut self, pos: usize, extend: bool) {
        let pos = pos.min(self.model.len_bytes());
        if extend {
            self.selection.extend_to(pos);
        } else {
            self.cancel_composition();
            self.selection.move_to(pos);
        }
    }

    pub fn move_left(&mut self, extend: bool) {
        let p = self.model.prev_grapheme(self.selection.caret);
        self.move_caret(p, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        let p = self.model.next_grapheme(self.selection.caret);
        self.move_caret(p, extend);
    }

    pub fn move_word_left(&mut self, extend: bool) {
        let p = self.model.prev_word(self.selection.caret);
        self.move_caret(p, extend);
    }

    pub fn move_word_right(&mut self, extend: bool) {
        let p = self.model.next_word(self.selection.caret);
        self.move_caret(p, extend);
    }

    pub fn move_to_start(&mut self, extend: bool) {
        self.move_caret(0, extend);
    }

    pub fn move_to_end(&mut self, extend: bool) {
        self.move_caret(self.model.len_bytes(), extend);
    }

    pub fn select_all(&mut self) {
        self.selection = Selection::new(0, self.model.len_bytes());
    }

    // ---- undo / redo ----

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(rec) = self.undo_stack.pop() else {
            return false;
        };
        self.cancel_composition();
        match &rec.kind {
            EditKind::Insert { pos, text } => {
                let end = pos + text.len();
                self.model.delete(*pos..end);
            }
            EditKind::Delete { pos, text } => {
                self.model.insert(*pos, text);
            }
            EditKind::Replace {
                pos,
                deleted,
                inserted,
            } => {
                let end = pos + inserted.len();
                self.model.delete(*pos..end);
                self.model.insert(*pos, deleted);
            }
        }
        self.selection = rec.before;
        self.selection.clamp(self.model.len_bytes());
        self.redo_stack.push(rec);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(rec) = self.redo_stack.pop() else {
            return false;
        };
        self.cancel_composition();
        match &rec.kind {
            EditKind::Insert { pos, text } => {
                self.model.insert(*pos, text);
            }
            EditKind::Delete { pos, text } => {
                let end = pos + text.len();
                self.model.delete(*pos..end);
            }
            EditKind::Replace {
                pos,
                deleted,
                inserted,
            } => {
                let end = pos + deleted.len();
                self.model.delete(*pos..end);
                self.model.insert(*pos, inserted);
            }
        }
        self.selection = rec.after;
        self.selection.clamp(self.model.len_bytes());
        self.undo_stack.push(rec);
        true
    }

    // ---- IME composition ----

    /// True while a preedit is active.
    pub fn is_composing(&self) -> bool {
        self.ime.is_some()
    }

    /// Set or replace the preedit. `cursor` is a byte range inside the
    /// preedit text (often empty). The preedit is inserted at the current
    /// caret, replacing any selection; selection collapses into the preedit
    /// cursor. The replaced selection is held UNCOMMITTED in the composition:
    /// cancel restores it losslessly, commit folds it into one undoable
    /// Replace, and a keystroke flushes it as a Delete before the new edit.
    pub fn set_composition(&mut self, text: &str, cursor: std::ops::Range<usize>) {
        let text = self.apply_line_policy(text);
        // Superseded active preedit: keep its replaced text only when the
        // new preedit continues at the same anchor (a preedit update);
        // otherwise restore first so nothing is silently lost.
        let carried = self.ime.take().and_then(|old| {
            let caret = self.selection.caret.min(self.model.len_bytes());
            if old.start == caret {
                Some(old)
            } else {
                self.restore_replaced(old.replaced, old.start);
                None
            }
        });
        let replaced = if !self.selection.is_caret() {
            let range = self.selection.range();
            let before = self.selection;
            let deleted = self.model.delete(range.clone());
            self.selection.collapse_to(range.start);
            self.redo_stack.clear();
            Some(Replaced {
                pos: range.start,
                deleted,
                before,
            })
        } else {
            carried.and_then(|old| old.replaced)
        };
        let start = self.selection.caret.min(self.model.len_bytes());
        let len = text.len();
        let cs = cursor.start.min(len);
        let ce = cursor.end.min(len);
        self.ime = Some(Composition {
            start,
            text,
            cursor: cs..ce,
            replaced,
        });
    }

    /// Commit the preedit into the text as one undoable step (a Replace when
    /// the preedit covered a selection, else an Insert).
    /// Returns false when nothing was composing.
    pub fn commit_composition(&mut self) -> bool {
        let Some(c) = self.ime.take() else {
            return false;
        };
        if c.text.is_empty() {
            // Empty preedit committed nothing: give back what the preedit
            // displaced, with no history change.
            self.restore_replaced(c.replaced, c.start);
            return true;
        }
        let ins = self.model.insert(c.start, &c.text);
        self.selection.collapse_to(ins.end);
        let after = self.selection;
        match c.replaced {
            Some(r) => self.push_record(EditRecord {
                kind: EditKind::Replace {
                    pos: r.pos,
                    deleted: r.deleted,
                    inserted: c.text,
                },
                before: r.before,
                after,
            }),
            None => self.push_record(EditRecord {
                kind: EditKind::Insert {
                    pos: c.start,
                    text: c.text,
                },
                before: Selection::caret_at(c.start),
                after,
            }),
        }
        self.redo_stack.clear();
        true
    }

    /// Drop the preedit, losslessly restoring any selection it displaced.
    /// No history change: uncommitted text was never real.
    pub fn cancel_composition(&mut self) -> bool {
        let Some(c) = self.ime.take() else {
            return false;
        };
        self.restore_replaced(c.replaced, c.start);
        true
    }

    /// Byte range of the preedit in *display* coordinates
    /// (`display_text()`), for IME candidate-window positioning and
    /// underline rendering. `None` when not composing.
    pub fn composition_range(&self) -> Option<std::ops::Range<usize>> {
        self.ime.as_ref().map(|c| c.start..c.start + c.text.len())
    }

    /// Caret position for IME purposes: the preedit cursor when composing,
    /// else the normal caret. In *display* coordinates.
    pub fn ime_caret(&self) -> usize {
        match &self.ime {
            Some(c) => c.start + c.cursor.end.min(c.text.len()),
            None => self.selection.caret,
        }
    }

    // ---- internals ----

    fn apply_line_policy(&self, s: &str) -> String {
        if self.line_mode == LineMode::Single {
            s.replace(['\n', '\r'], " ")
        } else {
            s.replace('\r', "")
        }
    }

    fn push_record(&mut self, rec: EditRecord) {
        self.undo_stack.push(rec);
        while self.undo_stack.len() > self.undo_limit {
            self.undo_stack.remove(0);
        }
    }

    /// Merge a typed char into the previous insert record when adjacent.
    fn try_merge_insert(&mut self, pos: usize, s: &str, before: Selection, caret: usize) -> bool {
        if s.chars().count() != 1 {
            return false;
        }
        let Some(last) = self.undo_stack.last_mut() else {
            return false;
        };
        match &mut last.kind {
            EditKind::Insert { pos: p, text } if *p + text.len() == pos => {
                text.push_str(s);
                last.after = Selection::caret_at(caret);
                let _ = before;
                true
            }
            _ => false,
        }
    }

    /// Keystroke edits replace an active preedit: the preedit itself was
    /// never committed text (no record for dropping it), but a selection it
    /// displaced becomes permanent here, so flush that as one Delete record
    /// before the new edit proceeds.
    fn commit_composition_if_any_without_record(&mut self) {
        let Some(c) = self.ime.take() else {
            return;
        };
        if let Some(r) = c.replaced {
            self.selection
                .collapse_to(r.pos.min(self.model.len_bytes()));
            let after = self.selection;
            self.push_record(EditRecord {
                kind: EditKind::Delete {
                    pos: r.pos,
                    text: r.deleted,
                },
                before: r.before,
                after,
            });
        } else {
            self.selection
                .collapse_to(c.start.min(self.model.len_bytes()));
        }
        self.redo_stack.clear();
    }

    /// Restore text displaced by a preedit (cancel / empty-commit /
    /// superseded preedit), or collapse to `fallback` when nothing was
    /// displaced. Never touches undo history.
    fn restore_replaced(&mut self, replaced: Option<Replaced>, fallback: usize) {
        match replaced {
            Some(r) => {
                self.model.insert(r.pos, &r.deleted);
                self.selection = r.before;
                self.selection.clamp(self.model.len_bytes());
            }
            None => self
                .selection
                .collapse_to(fallback.min(self.model.len_bytes())),
        }
    }
}

/// Snap a clamped byte index to the nearest grapheme-cluster boundary.
/// Ties snap to the previous boundary. The model already guarantees
/// boundary safety on edit; this is the safety net for raw UI input
/// (e.g. hit-test coordinates) passed to the public setters.
fn snap_nearest(model: &TextModel, at: usize) -> usize {
    if model.is_grapheme_boundary(at) {
        return at;
    }
    let prev = model.prev_grapheme(at);
    let next = model.next_grapheme(at);
    if at - prev <= next - at {
        prev
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_delete_removes_whole_cluster() {
        let mut ed = TextEditor::with_text("aéb", LineMode::Single);
        ed.move_to_end(false);
        ed.move_left(false); // caret after "b" -> before "b"? "aéb": move left from end
        ed.move_left(false); // before the e+combining cluster
        assert!(ed.delete_forward());
        assert_eq!(ed.text(), "ab");
    }

    #[test]
    fn emoji_deleted_as_one() {
        let mut ed = TextEditor::with_text("a👨‍👩‍👧b", LineMode::Single);
        ed.move_to_end(false);
        ed.move_left(false);
        ed.move_left(false);
        ed.delete_forward();
        assert_eq!(ed.text(), "ab");
    }

    #[test]
    fn single_line_strips_newlines() {
        let mut ed = TextEditor::new(LineMode::Single);
        ed.insert("a\nb\rc");
        assert_eq!(ed.text(), "a b c");
    }

    #[test]
    fn multiline_keeps_newlines() {
        let mut ed = TextEditor::new(LineMode::Multi);
        ed.insert("a\nb");
        assert_eq!(ed.text(), "a\nb");
    }

    #[test]
    fn insert_replaces_selection() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true);
        assert_eq!(ed.selected_text(), Some("world"));
        ed.insert("there");
        assert_eq!(ed.text(), "hello there");
    }

    #[test]
    fn undo_redo_insert() {
        let mut ed = TextEditor::new(LineMode::Single);
        ed.insert("a");
        ed.insert("b");
        assert_eq!(ed.text(), "ab");
        assert!(ed.undo());
        // 'a' and 'b' merged into one record (adjacent single-char inserts).
        assert_eq!(ed.text(), "");
        assert!(ed.redo());
        assert_eq!(ed.text(), "ab");
    }

    #[test]
    fn undo_redo_delete() {
        let mut ed = TextEditor::with_text("hello", LineMode::Single);
        ed.move_to_end(false);
        ed.delete_backward();
        assert_eq!(ed.text(), "hell");
        assert!(ed.undo());
        assert_eq!(ed.text(), "hello");
        assert!(ed.redo());
        assert_eq!(ed.text(), "hell");
    }

    #[test]
    fn typing_merges_but_replace_does_not() {
        let mut ed = TextEditor::new(LineMode::Single);
        ed.insert("ab");
        assert_eq!(ed.undo_len(), 1);
        ed.insert("c");
        assert_eq!(ed.undo_len(), 1); // merged
        ed.select_all();
        ed.insert("z");
        assert_eq!(ed.undo_len(), 2); // one Replace record
        assert_eq!(ed.text(), "z");
    }

    #[test]
    fn replace_selection_is_single_undo_restoring_content_and_selection() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true); // selects "world", anchor=11 caret=6
        let sel_before = ed.selection();
        assert_eq!(ed.selected_text(), Some("world"));
        ed.insert("there");
        assert_eq!(ed.text(), "hello there");
        assert_eq!(ed.undo_len(), 1, "replace is one user operation");

        assert!(ed.undo());
        assert_eq!(ed.text(), "hello world");
        assert_eq!(
            ed.selection(),
            sel_before,
            "undo restores the selection too"
        );
        assert!(ed.redo());
        assert_eq!(ed.text(), "hello there");
        assert!(ed.selection().is_caret());
    }

    #[test]
    fn mode_change_to_single_clears_history_when_folding() {
        let mut ed = TextEditor::new(LineMode::Multi);
        ed.insert("a\nb");
        ed.insert("c"); // "a\nbc", history present
        assert!(ed.can_undo());
        ed.set_line_mode(LineMode::Single);
        assert_eq!(ed.text(), "a bc");
        assert!(
            !ed.can_undo() && !ed.can_redo(),
            "folded byte positions invalidate history; drop it, never replay newlines"
        );
        assert!(ed.text().chars().all(|c| c != '\n'));
    }

    #[test]
    fn mode_change_to_single_keeps_history_without_newlines() {
        let mut ed = TextEditor::with_text("abc", LineMode::Multi);
        ed.move_to_end(false);
        ed.insert("d");
        assert_eq!(ed.text(), "abcd");
        ed.set_line_mode(LineMode::Single); // no folding: positions valid
        assert!(ed.can_undo());
        assert!(ed.undo());
        assert_eq!(ed.text(), "abc");
    }

    #[test]
    fn ime_over_selection_commit_is_single_undo() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true); // selects "world"
        let sel_before = ed.selection();
        ed.set_composition("ワールド", 0..12);
        assert!(ed.is_composing());
        // Committed text untouched; display shows the preedit in its place.
        assert_eq!(ed.text(), "hello ");
        assert_eq!(ed.display_text(), "hello ワールド");

        assert!(ed.commit_composition());
        assert_eq!(ed.text(), "hello ワールド");
        assert_eq!(ed.undo_len(), 1, "commit is one user operation");
        assert!(ed.undo());
        assert_eq!(ed.text(), "hello world");
        assert_eq!(ed.selection(), sel_before);
        assert!(ed.redo());
        assert_eq!(ed.text(), "hello ワールド");
    }

    #[test]
    fn ime_over_selection_cancel_is_lossless() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true); // selects "world"
        ed.set_composition("あ", 0..3);
        assert!(ed.is_composing());
        assert!(ed.cancel_composition());
        assert!(!ed.is_composing());
        assert_eq!(ed.text(), "hello world", "cancel restores displaced text");
        assert_eq!(
            ed.selected_text(),
            Some("world"),
            "cancel restores the selection too"
        );
        assert!(!ed.can_undo(), "uncommitted composition leaves no history");
    }

    #[test]
    fn ime_update_then_cancel_restores_original() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true);
        ed.set_composition("あ", 0..3);
        ed.set_composition("あい", 0..6); // preedit update at same anchor
        assert_eq!(ed.display_text(), "hello あい");
        assert!(ed.cancel_composition());
        assert_eq!(ed.text(), "hello world");
        assert_eq!(ed.selected_text(), Some("world"));
    }

    #[test]
    fn keystroke_during_compose_flushes_displaced_text_to_history() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true); // selects "world"
        ed.set_composition("あ", 0..3);
        ed.insert("X"); // replaces the uncommitted preedit
        assert_eq!(ed.text(), "hello X");
        assert!(!ed.is_composing());
        // Two records: the flushed selection deletion + the typed insert.
        assert!(ed.undo());
        assert_eq!(ed.text(), "hello ");
        assert!(ed.undo());
        assert_eq!(ed.text(), "hello world");
        assert_eq!(ed.selected_text(), Some("world"));
    }

    #[test]
    fn invalid_positions_are_grapheme_safe() {
        use crate::model::TextModel;
        // Mid-cluster, mid-char and out-of-range indices never panic and
        // never split clusters or scalar values.
        let mut m = TextModel::new("ae\u{301}x");
        assert!(!m.is_grapheme_boundary(2)); // inside "e + combining mark"
        assert!(!m.is_grapheme_boundary(3)); // inside the combining mark bytes
        m.delete(2..3); // snaps outward to the whole cluster
        assert_eq!(m.text(), "ax");
        m.insert(usize::MAX, "!");
        assert!(m.text().ends_with('!'));
        m.delete(usize::MAX..usize::MAX); // clamped to a trailing no-op
        assert_eq!(m.next_grapheme(usize::MAX), m.len_bytes());
        // End of text: Left moves into the last cluster, not past the end.
        assert_eq!(m.prev_grapheme(usize::MAX), m.len_bytes() - "!".len());
        assert!(m.is_grapheme_boundary(0));

        // Editor no-ops on an empty document.
        let mut ed = TextEditor::new(LineMode::Single);
        assert!(!ed.delete_backward());
        assert!(!ed.delete_forward());
        assert!(!ed.undo());
        assert!(!ed.redo());
        ed.move_left(false);
        ed.move_right(false);
        assert_eq!(ed.selection(), Selection::caret_at(0));

        // Oversized IME cursor clamps into the preedit.
        let mut ed2 = TextEditor::with_text("hi", LineMode::Single);
        ed2.move_to_end(false);
        ed2.set_composition("あ", 0..999);
        assert_eq!(ed2.ime_caret(), 2 + "あ".len());
        assert!(ed2.commit_composition());
        assert_eq!(ed2.text(), "hiあ");
    }

    #[test]
    fn ime_compose_commit_cancel() {
        let mut ed = TextEditor::with_text("hi", LineMode::Single);
        ed.move_to_end(false);
        ed.set_composition("あ", 0..1);
        assert!(ed.is_composing());
        assert_eq!(ed.display_text(), "hiあ");
        assert_eq!(ed.composition_range(), Some(2..5));
        assert!(ed.commit_composition());
        assert_eq!(ed.text(), "hiあ");
        assert!(!ed.is_composing());
        assert!(ed.undo()); // commit is one undo step
        assert_eq!(ed.text(), "hi");

        ed.set_composition("x", 0..1);
        assert!(ed.cancel_composition());
        assert_eq!(ed.text(), "hi");
    }

    #[test]
    fn undo_limit_bounds_history() {
        let mut ed = TextEditor::new(LineMode::Single);
        ed.set_undo_limit(3);
        ed.insert("a b c d e"); // spaces break merging -> several records
        assert!(ed.undo_len() <= 3, "undo stack exceeded limit");
    }
}

#[cfg(test)]
mod setter_tests {
    use super::super::*;
    use crate::selection::Selection;

    #[test]
    fn set_caret_clamps_and_snaps_invalid_positions() {
        let mut ed = TextEditor::with_text("aéx", LineMode::Single);
        // Byte 2 is inside the "e + combining mark" cluster (1..4).
        ed.set_caret(2);
        assert_eq!(ed.selection(), Selection::caret_at(1), "snaps to nearest");
        // Byte 3 is also inside the cluster, equidistant-ish: ties go down.
        ed.set_caret(3);
        assert!(ed.selection().caret == 1 || ed.selection().caret == 4);
        ed.set_caret(usize::MAX);
        assert_eq!(ed.selection(), Selection::caret_at(ed.text().len()));
        // No history change from placement.
        assert!(!ed.can_undo());
        assert_eq!(ed.undo_len(), 0);
    }

    #[test]
    fn set_caret_snaps_emoji_as_one() {
        let mut ed = TextEditor::with_text("a👨‍👧b", LineMode::Single);
        let emoji_start = 1;
        let emoji_end = "a👨‍👧".len();
        assert!(emoji_end - emoji_start > 2);
        ed.set_caret(emoji_start + 2); // mid-sequence
        let c = ed.selection().caret;
        assert!(
            c == emoji_start || c == emoji_end,
            "lands on a cluster edge, got {c}"
        );
        // Typing there keeps the emoji intact.
        ed.insert("!");
        assert!(ed.text().starts_with("a👨‍👧") || ed.text().contains("👨‍👧"));
    }

    #[test]
    fn set_selection_preserves_anchor_caret_extend_semantics() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.set_selection(Selection::new(11, 6)); // anchor right, caret left
        assert_eq!(ed.selection(), Selection::new(11, 6));
        assert_eq!(ed.selected_text(), Some("world"));
        // Reversed order is preserved, not normalized.
        ed.set_selection(Selection::new(2, 9));
        assert_eq!(ed.selection(), Selection::new(2, 9));
        assert_eq!(ed.selected_text(), Some("llo wor"));
        // Out-of-range ends clamp.
        ed.set_selection(Selection::new(usize::MAX, 0));
        assert_eq!(ed.selection(), Selection::new(ed.text().len(), 0));
        assert!(!ed.can_undo(), "placement never records history");
    }

    #[test]
    fn setters_cancel_composition_losslessly_without_history() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true); // selects "world"
        ed.set_composition("あ", 0..3);
        assert!(ed.is_composing());
        // Click elsewhere: preedit drops, displaced text is restored
        // losslessly, and the click position wins (restored selection is
        // then replaced by the placement, as with a plain cancel+click).
        ed.set_caret(0);
        assert!(!ed.is_composing());
        assert_eq!(ed.text(), "hello world");
        assert_eq!(ed.selection(), Selection::caret_at(0));
        assert!(!ed.can_undo());
        // set_selection during composition behaves the same.
        let mut ed2 = TextEditor::with_text("hello world", LineMode::Single);
        ed2.move_to_end(false);
        ed2.move_word_left(true);
        ed2.set_composition("あ", 0..3);
        ed2.set_selection(Selection::new(0, 5));
        assert!(!ed2.is_composing());
        assert_eq!(ed2.text(), "hello world");
        assert_eq!(ed2.selected_text(), Some("hello"));
    }

    #[test]
    fn set_selection_mid_cluster_snaps_both_ends_independently() {
        let mut ed = TextEditor::with_text("xáy", LineMode::Single);
        // "á" cluster spans bytes 1..4. Select 2..3 (fully inside).
        ed.set_selection(Selection::new(2, 3));
        let r = ed.selection().range();
        // Both ends snap out to whole-cluster edges; never a split cluster.
        assert!(r.start == 1 && r.end == 4, "got {r:?}");
        assert_eq!(ed.selected_text(), Some("á"));
    }
}
