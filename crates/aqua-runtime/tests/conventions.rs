use aqua_core::{EditingConventions, InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId};
use aqua_protocol::{Machine, RuntimeInput, RuntimeOutput, ShellCommand, ShellEvent};
use aqua_runtime::Runtime;
use aqua_text::{Selection, SharedFonts, SharedRuns};
use aqua_ui::element::ui::Text as UiText;
use aqua_ui::style::{px, Style};

mod support;

const COMMAND: Modifiers = Modifiers::LOGO;
const OPTION: Modifiers = Modifiers::ALT;
const CONTROL: Modifiers = Modifiers::CONTROL;

struct Field {
    runtime: Runtime,
    node: NodeId,
}

impl Field {
    fn new(conventions: EditingConventions, text: &'static str, caret: usize) -> Self {
        let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
        runtime.set_editing_conventions(conventions);
        runtime
            .mount(move |_| {
                UiText::builder()
                    .arg(text)
                    .editable(true)
                    .multiline(true)
                    .style(Style::new().width(px(600.0)).height(px(80.0)))
                    .build()
            })
            .unwrap();
        runtime.set_viewport(700.0, 200.0, 1.0);
        runtime.build_frame().unwrap();
        let node = runtime.tree().root().unwrap();
        runtime.tree_mut().set_focused(Some(node));
        runtime
            .tree_mut()
            .set_text_selection(node, Some(Selection::new(caret, caret)));
        Self { runtime, node }
    }

    fn press(&mut self, key: Key, modifiers: Modifiers) -> bool {
        self.runtime.handle_input(&chord(key, modifiers))
    }

    fn step(&mut self, key: Key, modifiers: Modifiers) -> RuntimeOutput {
        self.runtime.step(RuntimeInput::Shell(ShellEvent::Input {
            window: support::window(),
            event: chord(key, modifiers),
        }))
    }

    fn text(&self) -> String {
        self.runtime.tree().editor_snapshot(self.node).unwrap().text
    }

    fn selection(&self) -> (usize, usize) {
        let selection = self
            .runtime
            .tree()
            .editor_snapshot(self.node)
            .unwrap()
            .selection;
        (selection.anchor, selection.caret)
    }
}

fn chord(key: Key, modifiers: Modifiers) -> InputEvent {
    InputEvent::KeyDown(KeyEvent {
        key,
        modifiers,
        repeat: false,
    })
}

fn named(key: NamedKey) -> Key {
    Key::Named(key)
}

fn clipboard_writes(output: &RuntimeOutput) -> Vec<String> {
    output
        .shell
        .iter()
        .filter_map(|command| match command {
            ShellCommand::ClipboardWrite { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn clipboard_reads(output: &RuntimeOutput) -> usize {
    output
        .shell
        .iter()
        .filter(|command| matches!(command, ShellCommand::ClipboardRead { .. }))
        .count()
}

#[test]
fn command_c_copies_under_mac_conventions_and_types_nothing() {
    let mut field = Field::new(EditingConventions::Mac, "draft", 0);
    field.step(Key::Char('a'), COMMAND);
    assert_eq!(field.selection(), (0, 5));
    assert!(clipboard_writes(&field.step(Key::Char('c'), CONTROL)).is_empty());
    assert_eq!(
        clipboard_writes(&field.step(Key::Char('c'), COMMAND)),
        vec!["draft".to_string()]
    );
    assert_eq!(clipboard_reads(&field.step(Key::Char('v'), COMMAND)), 1);
    assert_eq!(field.text(), "draft");
    assert_eq!(
        clipboard_writes(&field.step(Key::Char('x'), COMMAND)),
        vec!["draft".to_string()]
    );
    assert_eq!(field.text(), "");
}

#[test]
fn option_left_moves_by_word_under_mac_conventions() {
    let mut field = Field::new(EditingConventions::Mac, "hello brave world", 17);
    assert!(field.press(named(NamedKey::Left), OPTION));
    assert_eq!(field.selection(), (12, 12));
    assert!(field.press(named(NamedKey::Left), OPTION));
    assert_eq!(field.selection(), (6, 6));
    assert!(field.press(named(NamedKey::Right), OPTION.union(Modifiers::SHIFT)));
    assert_eq!(field.selection(), (6, 12));
    assert!(!field.press(named(NamedKey::Left), CONTROL));
    assert_eq!(field.selection(), (6, 12));
}

#[test]
fn command_left_moves_to_line_start_under_mac_conventions() {
    let mut field = Field::new(EditingConventions::Mac, "first line\nsecond line", 14);
    assert!(field.press(named(NamedKey::Left), COMMAND));
    assert_eq!(field.selection(), (11, 11));
    assert!(field.press(named(NamedKey::Right), COMMAND));
    assert_eq!(field.selection(), (22, 22));
    assert!(field.press(named(NamedKey::Up), COMMAND));
    assert_eq!(field.selection(), (0, 0));
    assert!(field.press(named(NamedKey::Down), COMMAND.union(Modifiers::SHIFT)));
    assert_eq!(field.selection(), (0, 22));
}

#[test]
fn option_and_command_backspace_delete_a_word_and_to_line_start_under_mac_conventions() {
    let mut field = Field::new(
        EditingConventions::Mac,
        "first line\nsecond brave words",
        29,
    );
    assert!(field.press(named(NamedKey::Backspace), OPTION));
    assert_eq!(field.text(), "first line\nsecond brave ");
    assert!(field.press(named(NamedKey::Backspace), COMMAND));
    assert_eq!(field.text(), "first line\n");
    assert!(field.press(Key::Char('z'), COMMAND));
    assert_eq!(field.text(), "first line\nsecond brave ");
    assert!(field.press(Key::Char('z'), COMMAND.union(Modifiers::SHIFT)));
    assert_eq!(field.text(), "first line\n");
    assert!(!field.press(Key::Char('y'), COMMAND));
    assert_eq!(field.text(), "first line\n");
}

#[test]
fn pc_conventions_are_unchanged() {
    let mut field = Field::new(EditingConventions::Pc, "first line\nsecond brave words", 29);
    for (key, modifiers) in [
        (named(NamedKey::Left), OPTION),
        (named(NamedKey::Left), COMMAND),
        (named(NamedKey::Up), COMMAND),
        (named(NamedKey::Backspace), COMMAND),
        (named(NamedKey::Backspace), OPTION),
        (Key::Char('a'), COMMAND),
        (Key::Char('z'), COMMAND),
    ] {
        assert!(!field.press(key, modifiers));
    }
    assert_eq!(field.text(), "first line\nsecond brave words");
    assert_eq!(field.selection(), (29, 29));
    assert!(clipboard_writes(&field.step(Key::Char('c'), COMMAND)).is_empty());
    assert!(field.press(named(NamedKey::Left), CONTROL));
    assert_eq!(field.selection(), (24, 24));
    assert!(field.press(named(NamedKey::Home), Modifiers::NONE));
    assert_eq!(field.selection(), (11, 11));
    assert!(field.press(named(NamedKey::End), Modifiers::SHIFT));
    assert_eq!(field.selection(), (11, 29));
    assert!(field.press(named(NamedKey::Home), CONTROL));
    assert_eq!(field.selection(), (0, 0));
    assert!(field.press(named(NamedKey::End), CONTROL));
    assert_eq!(field.selection(), (29, 29));
    assert!(field.press(named(NamedKey::Backspace), CONTROL));
    assert_eq!(field.text(), "first line\nsecond brave ");
    assert!(field.press(Key::Char('z'), CONTROL));
    assert_eq!(field.text(), "first line\nsecond brave words");
    assert!(field.press(Key::Char('y'), CONTROL));
    assert_eq!(field.text(), "first line\nsecond brave ");
    assert!(field.press(Key::Char('a'), CONTROL));
    assert_eq!(field.selection(), (0, 24));
    assert_eq!(
        clipboard_writes(&field.step(Key::Named(NamedKey::Insert), CONTROL)),
        vec!["first line\nsecond brave ".to_string()]
    );
}
