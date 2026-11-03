use aqua_text::{LineMode, Selection, TextEditor};

#[test]
fn arrows_collapse_ranges_without_skipping_a_grapheme() {
    let mut editor = TextEditor::with_text("a👨‍👩‍👧b", LineMode::Multi);
    let end = editor.text().len() - 1;
    editor.set_selection(Selection::new(1, end));
    editor.move_left(false);
    assert_eq!(editor.selection(), Selection::caret_at(1));
    editor.set_selection(Selection::new(end, 1));
    editor.move_right(false);
    assert_eq!(editor.selection(), Selection::caret_at(end));
}

#[test]
fn word_deletion_restores_text_and_original_caret_on_undo() {
    let mut editor = TextEditor::with_text("abc.def  next", LineMode::Multi);
    editor.set_caret(0);
    assert!(editor.delete_word_forward());
    assert_eq!(editor.text(), ".def  next");
    assert!(editor.undo());
    assert_eq!(editor.text(), "abc.def  next");
    assert_eq!(editor.selection(), Selection::caret_at(0));
    editor.move_to_end(false);
    assert!(editor.delete_word_backward());
    assert_eq!(editor.text(), "abc.def  ");
    assert!(editor.undo());
    assert_eq!(editor.selection().caret, editor.text().len());
}

#[test]
fn movement_and_paste_separate_typing_history() {
    let mut editor = TextEditor::new(LineMode::Multi);
    editor.insert("a");
    editor.insert("b");
    editor.move_left(false);
    editor.move_right(false);
    editor.insert("c");
    editor.insert_paste("d");
    editor.insert("e");
    assert!(editor.undo());
    assert_eq!(editor.text(), "abcd");
    assert!(editor.undo());
    assert_eq!(editor.text(), "abc");
    assert!(editor.undo());
    assert_eq!(editor.text(), "ab");
    assert!(editor.undo());
    assert_eq!(editor.text(), "");
}

#[test]
fn undo_inserted_combining_marks_keeps_original_base() {
    let mut editor = TextEditor::with_text("a", LineMode::Multi);
    editor.move_to_end(false);
    editor.insert("\u{301}");
    assert!(editor.undo());
    assert_eq!(editor.text(), "a");
    assert!(editor.redo());
    assert_eq!(editor.text(), "a\u{301}");
}

#[test]
fn vertical_and_paragraph_movement_preserve_grapheme_column() {
    let mut editor = TextEditor::with_text("ab👩‍💻de\nx\nuvwxyz", LineMode::Multi);
    let at = "ab👩‍💻".len();
    editor.set_caret(at);
    editor.move_paragraph(true, true);
    assert_eq!(editor.selected_text(), Some("de\nx"));
    editor.move_paragraph(true, true);
    assert_eq!(editor.selection().caret, editor.text().len() - 3);
    editor.move_paragraph(false, false);
    editor.move_paragraph(false, false);
    assert_eq!(editor.selection(), Selection::caret_at(at));
}

#[test]
fn paste_normalizes_line_endings_once() {
    let mut editor = TextEditor::new(LineMode::Multi);
    editor.insert_paste("a\r\nb\rc");
    assert_eq!(editor.text(), "a\nb\nc");
    let mut single = TextEditor::new(LineMode::Single);
    single.insert_paste("a\r\nb\rc");
    assert_eq!(single.text(), "a b c");
}
