use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, Point};
use aqua_runtime::{PopupOptions, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::controls::SelectOption;
use aqua_ui::element::ui::{Box, Dropdown, Text};
use aqua_ui::overlay::OverlayId;
use aqua_ui::style::{px, Axis, Style};

fn key(key: NamedKey) -> InputEvent {
    InputEvent::KeyDown(KeyEvent {
        key: Key::Named(key),
        modifiers: Modifiers::NONE,
        repeat: false,
    })
}

fn rig() -> Runtime {
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        Box::new()
            .style(Style::new().axis(Axis::Column))
            .child(Text::new("base").build())
            .child(
                Dropdown::new("Engine")
                    .options(vec![
                        SelectOption::new("a", "Alpha"),
                        SelectOption::new("b", "Beta"),
                    ])
                    .selected(0)
                    .style(Style::new().width(px(220.0)).height(px(32.0)))
                    .build(),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(300.0, 200.0, 1.0);
    app.build_frame().unwrap();
    app
}

fn open(app: &mut Runtime, nested: bool) -> Vec<OverlayId> {
    let owner = app.tree().root().unwrap();
    let outer = app
        .open_popup(
            owner,
            Box::new()
                .style(
                    Style::new()
                        .width(px(80.0))
                        .height(px(60.0))
                        .axis(Axis::Column),
                )
                .child(Text::new("one").build())
                .child(Text::new("two").build())
                .build(),
            PopupOptions::menu().at(Point::new(10.0, 20.0)),
        )
        .unwrap();
    let mut out = vec![outer];
    if nested {
        app.build_frame().unwrap();
        let host_root = app.host().popup_root(outer).unwrap();
        out.push(
            app.open_popup(
                host_root,
                Box::new()
                    .style(Style::new().width(px(60.0)).height(px(30.0)))
                    .child(Text::new("inner").build())
                    .build(),
                PopupOptions::menu().at(Point::new(40.0, 30.0)),
            )
            .unwrap(),
        );
    }
    out
}

#[test]
fn layout_survives_the_frame_after_a_popup_closes() {
    let mut app = rig();
    for round in 0..4 {
        let popups = open(&mut app, round % 2 == 1);
        app.build_frame().unwrap();
        for popup in popups.into_iter() {
            app.close_popup(popup);
        }
        app.build_frame().unwrap();
        app.build_frame().unwrap();
    }
}

#[test]
fn dropdown_commit_and_dismiss_leave_a_layoutable_tree() {
    let mut app = rig();
    let select = app.tree().traverse().into_iter().find(|node| {
        matches!(
            app.tree().control(*node),
            Some(aqua_ui::controls::Control::Select(_))
        )
    });
    let select = select.unwrap();
    for _ in 0..3 {
        app.tree_mut().set_focused(Some(select));
        app.handle_input(&key(NamedKey::Enter));
        app.build_frame().unwrap();
        app.handle_input(&key(NamedKey::Down));
        app.handle_input(&key(NamedKey::Enter));
        app.build_frame().unwrap();
        app.build_frame().unwrap();
        app.tree_mut().set_focused(Some(select));
        app.handle_input(&key(NamedKey::Enter));
        app.build_frame().unwrap();
        app.handle_input(&key(NamedKey::Escape));
        app.build_frame().unwrap();
        app.build_frame().unwrap();
    }
}
