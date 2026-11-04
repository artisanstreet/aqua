use crate::layout::{CaretAffinity, CaretPosition};
use crate::model::TextModel;
use crate::selection::Selection;
use crate::style::LineMode;

#[derive(Clone, Debug)]
struct EditRecord {
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
    Replace {
        pos: usize,
        deleted: String,
        inserted: String,
    },
}

#[derive(Clone, Debug)]
struct Replaced {
    pos: usize,
    deleted: String,
    before: Selection,
}

#[derive(Clone, Debug)]
struct Composition {
    start: usize,
    text: String,
    cursor: std::ops::Range<usize>,
    replaced: Option<Replaced>,
}

pub struct TextEditor {
    model: TextModel,
    selection: Selection,
    line_mode: LineMode,
    ime: Option<Composition>,
    external_projection: bool,
    undo_stack: Vec<EditRecord>,
    redo_stack: Vec<EditRecord>,
    undo_limit: usize,
    merge_typing: bool,
    vertical_column: Option<usize>,
    caret_affinity: CaretAffinity,
}

impl TextEditor {
    pub fn new(line_mode: LineMode) -> Self {
        Self {
            model: TextModel::new(String::new()),
            selection: Selection::default(),
            line_mode,
            ime: None,
            external_projection: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_limit: 100,
            merge_typing: false,
            vertical_column: None,
            caret_affinity: CaretAffinity::Downstream,
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

    pub fn display_text(&self) -> String {
        if self.external_projection {
            return self.model.text().to_owned();
        }
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

    pub fn set_caret(&mut self, pos: usize) {
        self.set_selection(Selection::caret_at(pos));
    }

    pub fn set_selection(&mut self, selection: Selection) {
        self.cancel_composition();
        self.break_typing_group();
        let len = self.model.len_bytes();
        let anchor = snap_nearest(&self.model, selection.anchor.min(len));
        let caret = snap_nearest(&self.model, selection.caret.min(len));
        self.selection = Selection::new(anchor, caret);
    }

    pub fn caret_position(&self) -> CaretPosition {
        CaretPosition {
            byte: self.selection.caret,
            affinity: self.caret_affinity,
        }
    }

    pub fn move_caret_position(&mut self, position: CaretPosition, extend: bool) {
        self.move_caret_to(position.byte, extend);
        self.caret_affinity = position.affinity;
    }

    pub fn line_mode(&self) -> LineMode {
        self.line_mode
    }

    pub fn set_line_mode(&mut self, mode: LineMode) {
        self.break_typing_group();
        self.line_mode = mode;
        if mode == LineMode::Single {
            self.cancel_composition();
            if self.model.text().contains('\n') {
                let clean: String = self.model.text().replace('\n', " ");
                self.model.set_text(clean);
                self.selection.clamp(self.model.len_bytes());
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

    pub fn set_undo_limit(&mut self, limit: usize) {
        self.undo_limit = limit.max(1);
        while self.undo_stack.len() > self.undo_limit {
            self.undo_stack.remove(0);
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.break_typing_group();
        let text = self.apply_line_policy(text);
        self.model.set_text(text);
        self.selection.collapse_to(0);
        self.selection.clamp(self.model.len_bytes());
        self.ime = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    pub fn selected_text(&self) -> Option<&str> {
        if self.selection.is_caret() {
            return None;
        }
        let r = self.selection.range();
        self.model.text().get(r)
    }

    pub fn insert(&mut self, s: &str) -> usize {
        self.commit_composition_if_any_without_record();
        let s = self.apply_line_policy(s);
        if s.is_empty() && self.selection.is_caret() {
            return self.selection.caret;
        }
        self.vertical_column = None;
        self.caret_affinity = CaretAffinity::Downstream;
        let merge_next = !s.contains(char::is_whitespace);
        let before = self.selection;
        let range = self.selection.range();
        let deleted = self.model.text()[range.clone()].to_owned();
        self.model.replace_exact(range.clone(), &s);
        let caret = snap_nearest(&self.model, range.start + s.len());
        self.selection.collapse_to(caret);
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
        self.merge_typing = merge_next;
        self.redo_stack.clear();
        caret
    }

    pub fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) -> bool {
        if self.is_composing() || range.start > range.end || range.end > self.text().len() {
            return false;
        }
        let before = self.selection;
        self.set_selection(Selection::new(range.start, range.end));
        let revision = self.revision();
        self.insert_paste(text);
        if self.revision() == revision {
            self.set_selection(before);
            return false;
        }
        if let Some(record) = self.undo_stack.last_mut() {
            record.before = before;
        }
        true
    }

    pub fn delete_backward(&mut self) -> bool {
        self.break_typing_group();
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
        self.selection.collapse_to(snap_nearest(&self.model, prev));
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

    pub fn delete_forward(&mut self) -> bool {
        self.break_typing_group();
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
        self.selection.collapse_to(snap_nearest(&self.model, caret));
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

    pub fn delete_selection(&mut self) {
        self.break_typing_group();
        let range = self.selection.range();
        if range.is_empty() {
            return;
        }
        self.commit_composition_if_any_without_record();
        let before = self.selection;
        let removed = self.model.delete(range.clone());
        self.selection
            .collapse_to(snap_nearest(&self.model, range.start));
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

    pub fn move_caret_to(&mut self, pos: usize, extend: bool) {
        self.cancel_composition();
        self.break_typing_group();
        let pos = snap_nearest(&self.model, pos.min(self.model.len_bytes()));
        if extend {
            self.selection.extend_to(pos);
        } else {
            self.cancel_composition();
            self.selection.move_to(pos);
        }
    }

    pub fn move_left(&mut self, extend: bool) {
        self.cancel_composition();
        let p = if !extend && !self.selection.is_caret() {
            self.selection.range().start
        } else {
            self.model.prev_grapheme(self.selection.caret)
        };
        self.move_caret_to(p, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        self.cancel_composition();
        let p = if !extend && !self.selection.is_caret() {
            self.selection.range().end
        } else {
            self.model.next_grapheme(self.selection.caret)
        };
        self.move_caret_to(p, extend);
    }

    pub fn move_word_left(&mut self, extend: bool) {
        self.cancel_composition();
        let p = self.model.prev_word(self.selection.caret);
        self.move_caret_to(p, extend);
    }

    pub fn move_word_right(&mut self, extend: bool) {
        self.cancel_composition();
        let p = self.model.next_word(self.selection.caret);
        self.move_caret_to(p, extend);
    }

    pub fn move_to_start(&mut self, extend: bool) {
        self.move_caret_to(0, extend);
    }

    pub fn move_to_end(&mut self, extend: bool) {
        self.move_caret_to(self.model.len_bytes(), extend);
    }

    pub fn break_typing_group(&mut self) {
        self.caret_affinity = CaretAffinity::Downstream;
        self.merge_typing = false;
        self.vertical_column = None;
    }

    pub fn insert_paste(&mut self, text: &str) -> usize {
        self.break_typing_group();
        let caret = self.insert(text);
        self.break_typing_group();
        caret
    }

    pub fn delete_word_backward(&mut self) -> bool {
        self.delete_word(true)
    }

    pub fn delete_word_forward(&mut self) -> bool {
        self.delete_word(false)
    }

    fn delete_word(&mut self, backward: bool) -> bool {
        self.cancel_composition();
        let before = self.selection;
        if self.selection.is_caret() {
            let at = self.selection.caret;
            let target = if backward {
                self.model.prev_word(at)
            } else {
                self.model.next_word(at)
            };
            if target == at {
                return false;
            }
            self.selection.extend_to(target);
        }
        self.delete_selection();
        if let Some(record) = self.undo_stack.last_mut() {
            record.before = before;
        }
        true
    }

    pub fn move_line_boundary(&mut self, end: bool, extend: bool) {
        self.cancel_composition();
        let at = self.selection.caret;
        let text = self.text();
        let target = if end {
            text[at..].find('\n').map_or(text.len(), |i| at + i)
        } else {
            text[..at].rfind('\n').map_or(0, |i| i + 1)
        };
        self.move_caret_to(target, extend);
    }

    pub fn move_paragraph(&mut self, down: bool, extend: bool) {
        self.move_vertical(!down, extend);
    }

    pub fn move_vertical(&mut self, up: bool, extend: bool) {
        use unicode_segmentation::UnicodeSegmentation;
        self.cancel_composition();
        let at = self.selection.caret;
        let start = self.text()[..at].rfind('\n').map_or(0, |i| i + 1);
        let column = self
            .vertical_column
            .unwrap_or_else(|| self.text()[start..at].graphemes(true).count());
        let text = self.text();
        let current_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        let row = if up {
            start.checked_sub(1).map(|end| {
                let begin = text[..end].rfind('\n').map_or(0, |i| i + 1);
                begin..end
            })
        } else if current_end < text.len() {
            let begin = current_end + 1;
            Some(begin..text[begin..].find('\n').map_or(text.len(), |i| begin + i))
        } else {
            None
        };
        let target = row.map_or(if up { 0 } else { text.len() }, |row| {
            text[row.clone()]
                .grapheme_indices(true)
                .nth(column)
                .map_or(row.end, |(i, _)| row.start + i)
        });
        self.move_caret_to(target, extend);
        self.vertical_column = Some(column);
    }

    pub fn select_all(&mut self) {
        self.cancel_composition();
        self.break_typing_group();
        self.selection = Selection::new(0, self.model.len_bytes());
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        self.break_typing_group();
        let Some(rec) = self.undo_stack.pop() else {
            return false;
        };
        self.cancel_composition();
        match &rec.kind {
            EditKind::Insert { pos, text } => {
                let end = pos + text.len();
                self.model.replace_exact(*pos..end, "");
            }
            EditKind::Delete { pos, text } => {
                self.model.replace_exact(*pos..*pos, text);
            }
            EditKind::Replace {
                pos,
                deleted,
                inserted,
            } => {
                let end = pos + inserted.len();
                self.model.replace_exact(*pos..end, "");
                self.model.replace_exact(*pos..*pos, deleted);
            }
        }
        self.selection = rec.before;
        self.selection.clamp(self.model.len_bytes());
        self.redo_stack.push(rec);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.break_typing_group();
        let Some(rec) = self.redo_stack.pop() else {
            return false;
        };
        self.cancel_composition();
        match &rec.kind {
            EditKind::Insert { pos, text } => {
                self.model.replace_exact(*pos..*pos, text);
            }
            EditKind::Delete { pos, text } => {
                let end = pos + text.len();
                self.model.replace_exact(*pos..end, "");
            }
            EditKind::Replace {
                pos,
                deleted,
                inserted,
            } => {
                let end = pos + deleted.len();
                self.model.replace_exact(*pos..end, "");
                self.model.replace_exact(*pos..*pos, inserted);
            }
        }
        self.selection = rec.after;
        self.selection.clamp(self.model.len_bytes());
        self.undo_stack.push(rec);
        true
    }

    pub fn is_composing(&self) -> bool {
        self.ime.is_some()
    }

    pub fn set_external_projection(
        &mut self,
        text: &str,
        selection: Selection,
        marked: Option<std::ops::Range<usize>>,
    ) -> bool {
        if selection.anchor > text.len()
            || selection.caret > text.len()
            || !text.is_char_boundary(selection.anchor)
            || !text.is_char_boundary(selection.caret)
        {
            return false;
        }
        let ime = if let Some(range) = marked {
            if range.start > range.end
                || text.get(range.clone()).is_none()
                || selection.anchor < range.start
                || selection.anchor > range.end
                || selection.caret < range.start
                || selection.caret > range.end
            {
                return false;
            }
            Some(Composition {
                start: range.start,
                text: text[range.clone()].to_owned(),
                cursor: selection.anchor - range.start..selection.caret - range.start,
                replaced: None,
            })
        } else {
            None
        };
        self.model.set_text(text);
        self.selection = selection;
        self.ime = ime;
        self.external_projection = true;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.merge_typing = false;
        self.vertical_column = None;
        true
    }

    pub fn set_composition(&mut self, text: &str, cursor: std::ops::Range<usize>) {
        let text = self.apply_line_policy(text);
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
        let preedit = TextModel::new(text.clone());
        let cs = snap_nearest(&preedit, cursor.start.min(len));
        let ce = snap_nearest(&preedit, cursor.end.min(len));
        self.ime = Some(Composition {
            start,
            text,
            cursor: cs..ce,
            replaced,
        });
    }

    pub fn commit_composition(&mut self) -> bool {
        let Some(c) = self.ime.take() else {
            return false;
        };
        if c.text.is_empty() {
            self.restore_replaced(c.replaced, c.start);
            return true;
        }
        self.model.replace_exact(c.start..c.start, &c.text);
        self.selection
            .collapse_to(snap_nearest(&self.model, c.start + c.text.len()));
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

    pub fn cancel_composition(&mut self) -> bool {
        let Some(c) = self.ime.take() else {
            return false;
        };
        self.restore_replaced(c.replaced, c.start);
        true
    }

    pub fn composition_range(&self) -> Option<std::ops::Range<usize>> {
        self.ime.as_ref().map(|c| c.start..c.start + c.text.len())
    }

    pub fn ime_caret(&self) -> usize {
        match &self.ime {
            Some(c) => c.start + c.cursor.end.min(c.text.len()),
            None => self.selection.caret,
        }
    }

    fn apply_line_policy(&self, s: &str) -> String {
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        if self.line_mode == LineMode::Single {
            normalized.replace('\n', " ")
        } else {
            normalized
        }
    }

    fn push_record(&mut self, rec: EditRecord) {
        self.undo_stack.push(rec);
        while self.undo_stack.len() > self.undo_limit {
            self.undo_stack.remove(0);
        }
    }

    fn try_merge_insert(&mut self, pos: usize, s: &str, before: Selection, caret: usize) -> bool {
        if !self.merge_typing || s.chars().count() != 1 {
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

    fn restore_replaced(&mut self, replaced: Option<Replaced>, fallback: usize) {
        match replaced {
            Some(r) => {
                self.model.replace_exact(r.pos..r.pos, &r.deleted);
                self.selection = r.before;
                self.selection.clamp(self.model.len_bytes());
            }
            None => self
                .selection
                .collapse_to(fallback.min(self.model.len_bytes())),
        }
    }
}

fn snap_nearest(model: &TextModel, at: usize) -> usize {
    if model.is_grapheme_boundary(at) {
        return at;
    }
    let prev = model.prev_grapheme(at);
    let next = model.next_grapheme(at);
    if at - prev <= next - at { prev } else { next }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_delete_removes_whole_cluster() {
        let mut ed = TextEditor::with_text("aéb", LineMode::Single);
        ed.move_to_end(false);
        ed.move_left(false);
        ed.move_left(false);
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
        assert_eq!(ed.undo_len(), 1);
        ed.select_all();
        ed.insert("z");
        assert_eq!(ed.undo_len(), 2);
        assert_eq!(ed.text(), "z");
    }

    #[test]
    fn replace_selection_is_single_undo_restoring_content_and_selection() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true);
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
        ed.insert("c");
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
        ed.set_line_mode(LineMode::Single);
        assert!(ed.can_undo());
        assert!(ed.undo());
        assert_eq!(ed.text(), "abc");
    }

    #[test]
    fn ime_over_selection_commit_is_single_undo() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true);
        let sel_before = ed.selection();
        ed.set_composition("ワールド", 0..12);
        assert!(ed.is_composing());
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
        ed.move_word_left(true);
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
        ed.set_composition("あい", 0..6);
        assert_eq!(ed.display_text(), "hello あい");
        assert!(ed.cancel_composition());
        assert_eq!(ed.text(), "hello world");
        assert_eq!(ed.selected_text(), Some("world"));
    }

    #[test]
    fn keystroke_during_compose_flushes_displaced_text_to_history() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true);
        ed.set_composition("あ", 0..3);
        ed.insert("X");
        assert_eq!(ed.text(), "hello X");
        assert!(!ed.is_composing());
        assert!(ed.undo());
        assert_eq!(ed.text(), "hello ");
        assert!(ed.undo());
        assert_eq!(ed.text(), "hello world");
        assert_eq!(ed.selected_text(), Some("world"));
    }

    #[test]
    fn invalid_positions_are_grapheme_safe() {
        use crate::model::TextModel;
        let mut m = TextModel::new("ae\u{301}x");
        assert!(!m.is_grapheme_boundary(2));
        assert!(!m.is_grapheme_boundary(3));
        m.delete(2..3);
        assert_eq!(m.text(), "ax");
        m.insert(usize::MAX, "!");
        assert!(m.text().ends_with('!'));
        m.delete(usize::MAX..usize::MAX);
        assert_eq!(m.next_grapheme(usize::MAX), m.len_bytes());
        assert_eq!(m.prev_grapheme(usize::MAX), m.len_bytes() - "!".len());
        assert!(m.is_grapheme_boundary(0));

        let mut ed = TextEditor::new(LineMode::Single);
        assert!(!ed.delete_backward());
        assert!(!ed.delete_forward());
        assert!(!ed.undo());
        assert!(!ed.redo());
        ed.move_left(false);
        ed.move_right(false);
        assert_eq!(ed.selection(), Selection::caret_at(0));

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
        assert!(ed.undo());
        assert_eq!(ed.text(), "hi");

        ed.set_composition("x", 0..1);
        assert!(ed.cancel_composition());
        assert_eq!(ed.text(), "hi");
    }

    #[test]
    fn undo_limit_bounds_history() {
        let mut ed = TextEditor::new(LineMode::Single);
        ed.set_undo_limit(3);
        ed.insert("a b c d e");
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
        ed.set_caret(2);
        assert_eq!(ed.selection(), Selection::caret_at(1), "snaps to nearest");
        ed.set_caret(3);
        assert!(ed.selection().caret == 1 || ed.selection().caret == 4);
        ed.set_caret(usize::MAX);
        assert_eq!(ed.selection(), Selection::caret_at(ed.text().len()));
        assert!(!ed.can_undo());
        assert_eq!(ed.undo_len(), 0);
    }

    #[test]
    fn set_caret_snaps_emoji_as_one() {
        let mut ed = TextEditor::with_text("a👨‍👧b", LineMode::Single);
        let emoji_start = 1;
        let emoji_end = "a👨‍👧".len();
        assert!(emoji_end - emoji_start > 2);
        ed.set_caret(emoji_start + 2);
        let c = ed.selection().caret;
        assert!(
            c == emoji_start || c == emoji_end,
            "lands on a cluster edge, got {c}"
        );
        ed.insert("!");
        assert!(ed.text().starts_with("a👨‍👧") || ed.text().contains("👨‍👧"));
    }

    #[test]
    fn set_selection_preserves_anchor_caret_extend_semantics() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.set_selection(Selection::new(11, 6));
        assert_eq!(ed.selection(), Selection::new(11, 6));
        assert_eq!(ed.selected_text(), Some("world"));
        ed.set_selection(Selection::new(2, 9));
        assert_eq!(ed.selection(), Selection::new(2, 9));
        assert_eq!(ed.selected_text(), Some("llo wor"));
        ed.set_selection(Selection::new(usize::MAX, 0));
        assert_eq!(ed.selection(), Selection::new(ed.text().len(), 0));
        assert!(!ed.can_undo(), "placement never records history");
    }

    #[test]
    fn setters_cancel_composition_losslessly_without_history() {
        let mut ed = TextEditor::with_text("hello world", LineMode::Single);
        ed.move_to_end(false);
        ed.move_word_left(true);
        ed.set_composition("あ", 0..3);
        assert!(ed.is_composing());
        ed.set_caret(0);
        assert!(!ed.is_composing());
        assert_eq!(ed.text(), "hello world");
        assert_eq!(ed.selection(), Selection::caret_at(0));
        assert!(!ed.can_undo());
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
        ed.set_selection(Selection::new(2, 3));
        let r = ed.selection().range();
        assert!(r.start == 1 && r.end == 4, "got {r:?}");
        assert_eq!(ed.selected_text(), Some("á"));
    }
}
