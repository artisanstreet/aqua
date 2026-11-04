use aqua::{ui::Text, UiApp};
use aqua_core::{ImeEvent as Composition, InputEvent};
use aqua_protocol::{ImeEvent, Machine, RuntimeInput, Utf16Range};
use aqua_text::{SharedFonts, SharedRuns};

fn focused_field() -> (UiApp, aqua_reactive::ReadSignal<String>) {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let (read, write) = app.scope().signal(String::new());
    app.mount(|_| Text::builder().bind((read.clone(), write)).build())
        .unwrap();
    let node = app.tree().root().unwrap();
    app.tree_mut().set_focused(Some(node));
    (app, read)
}

fn compose(app: &mut UiApp, event: Composition) {
    app.step(RuntimeInput::Ime(ImeEvent::Composition(event)));
    app.sync().unwrap();
}

fn type_text(app: &mut UiApp, text: &str) {
    app.handle_input(&InputEvent::TextInput { text: text.into() });
    app.sync().unwrap();
}

#[test]
fn composition_commits_between_ordinary_text_without_duplicates() {
    let (mut app, read) = focused_field();
    type_text(&mut app, "a");
    assert_eq!(read.get(), "a");
    compose(&mut app, Composition::Start);
    compose(
        &mut app,
        Composition::Update {
            marked: "文".into(),
            cursor: Some((3, 3)),
        },
    );
    assert_eq!(read.get(), "a");
    compose(
        &mut app,
        Composition::Update {
            marked: String::new(),
            cursor: None,
        },
    );
    compose(&mut app, Composition::Commit { text: "文".into() });
    type_text(&mut app, "b");
    assert_eq!(read.get(), "a文b");
}

#[test]
fn cancelled_composition_leaves_the_model_untouched() {
    let (mut app, read) = focused_field();
    type_text(&mut app, "x");
    compose(&mut app, Composition::Start);
    compose(
        &mut app,
        Composition::Update {
            marked: "ni".into(),
            cursor: None,
        },
    );
    compose(&mut app, Composition::Cancel);
    assert_eq!(read.get(), "x");
    type_text(&mut app, "y");
    assert_eq!(read.get(), "xy");
}

#[test]
fn tsf_replace_edits_the_document_by_utf16_range() {
    let (mut app, read) = focused_field();
    type_text(&mut app, "hello");
    app.step(RuntimeInput::Ime(ImeEvent::Replace {
        range: Utf16Range::new(1, 4),
        text: "ipp".into(),
    }));
    app.sync().unwrap();
    assert_eq!(read.get(), "hippo");
}
