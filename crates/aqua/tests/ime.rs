use aqua::{ui::Text, UiApp};
use aqua_core::InputEvent;
use aqua_platform::events::{ime_is_composing, translate_ime_core};
use aqua_text::{SharedFonts, SharedRuns};
use winit::event::Ime;

#[test]
fn enabled_ime_accepts_ordinary_text_and_only_preedit_suppresses_native_duplicates() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let (read, write) = app.scope().signal(String::new());
    app.mount(|_| Text::builder().bind((read.clone(), write)).build())
        .unwrap();
    let node = app.tree().root().unwrap();
    app.tree_mut().set_focused(Some(node));
    let mut composing = false;
    let mut native_ime = |app: &mut UiApp, ime: Ime, text: &str| {
        composing = ime_is_composing(&ime);
        app.handle_input(&translate_ime_core(&ime));
        if !composing {
            app.handle_input(&InputEvent::TextInput { text: text.into() });
        }
        app.sync().unwrap();
    };
    native_ime(&mut app, Ime::Enabled, "a");
    assert_eq!(read.get(), "a");
    native_ime(
        &mut app,
        Ime::Preedit("文".into(), Some((3, 3))),
        "duplicate",
    );
    native_ime(&mut app, Ime::Preedit(String::new(), None), "");
    native_ime(&mut app, Ime::Commit("文".into()), "b");
    assert_eq!(read.get(), "a文b");
    native_ime(&mut app, Ime::Disabled, "c");
    assert_eq!(read.get(), "a文bc");
}
