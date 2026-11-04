use aqua::{Aqua, UiApp};
use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::tree::NodeKind;

fn pointer(at: Point, pressed: bool) -> InputEvent {
    let event = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: at,
        buttons: u8::from(pressed),
    };
    match pressed {
        true => InputEvent::PointerDown(event, PointerButton::Primary),
        false => InputEvent::PointerUp(event, PointerButton::Primary),
    }
}

fn settle(app: &mut UiApp) {
    app.sync().unwrap();
    app.build_frame().unwrap();
}

#[test]
fn characters_typed_one_by_one_into_a_field_whose_focus_rebuilds_the_view_arrive_in_order() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let (draft_r, draft_w) = app.scope().signal(String::new());
    let (focused_r, focused_w) = app.scope().signal(false);
    app.mount_reactive((focused_r,), move |(focused,)| {
        let focus_write = focused_w.clone();
        let mark = if focused { "focused" } else { "idle" };
        Aqua::Box()
            .child(Aqua::Text(mark).build())
            .child(
                Aqua::Text("")
                    .bind((draft_r.clone(), draft_w.clone()))
                    .placeholder("New task")
                    .on_focus(move |value| { focus_write.set_if_changed(value); })
                    .build(),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(400.0, 300.0, 1.0);
    settle(&mut app);
    let field = app
        .tree()
        .traverse()
        .into_iter()
        .find(|n| matches!(app.tree().get(*n).map(|r| &r.kind), Some(NodeKind::Text(t)) if t.is_editable()))
        .unwrap();
    let rect = app.tree().rect(field).unwrap();
    let at = Point::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
    app.handle_input(&pointer(at, true));
    app.handle_input(&pointer(at, false));
    for ch in "hello world".chars() {
        app.handle_input(&InputEvent::TextInput {
            text: ch.to_string(),
        });
    }
    settle(&mut app);
    let typed: Vec<String> = app
        .tree()
        .traverse()
        .into_iter()
        .filter_map(|n| match app.tree().get(n).map(|r| &r.kind) {
            Some(NodeKind::Text(t)) if t.is_editable() => Some(t.text().to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(typed, vec!["hello world".to_string()]);
}
