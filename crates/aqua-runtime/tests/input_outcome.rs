use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::Box;
use aqua_ui::input::{EventPhase, InputKind, InputResponse};

#[test]
fn handled_input_and_redraw_are_independent() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Box::new().build().focusable(true).on_input(
                InputKind::KeyDown,
                EventPhase::Bubble,
                |_| InputResponse::handled(),
            )
        })
        .unwrap();
    let node = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(node));
    let event = InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Left),
        modifiers: Modifiers::ALT,
        repeat: false,
    });
    let handled = runtime.step_input_with_default_outcome(&event, |_| None);
    assert!(handled.consumed);
    let unhandled = InputEvent::KeyDown(KeyEvent {
        key: Key::Char('x'),
        modifiers: Modifiers::NONE,
        repeat: false,
    });
    runtime
        .refresh(vec![Box::new().build().focusable(true)])
        .unwrap();
    let outcome = runtime.step_input_with_default_outcome(&unhandled, |_| Some(false));
    assert!(outcome.consumed);
    assert!(outcome.redraw.is_empty());
    let outcome = runtime.step_input_with_default_outcome(&unhandled, |_| None);
    assert!(!outcome.consumed);
}
