use aqua_core::Point;
use aqua_runtime::{PopupOptions, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::Box;
use aqua_ui::style::{px, Style};

#[test]
fn pointer_anchor_updates_survive_layout_and_closed_handles() {
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(200.0, 120.0, 1.0);
    app.mount(|_| Box::new().build()).unwrap();
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    let popup = app
        .open_popup(
            owner,
            Box::new()
                .style(Style::new().width(px(80.0)).height(px(40.0)))
                .build()
                .debug_selector("preview"),
            PopupOptions::menu().at(Point::new(10.0, 20.0)),
        )
        .unwrap();
    app.set_popup_uncontained(popup, true).unwrap();
    let moved = app
        .set_popup_pointer_anchor(popup, Point::new(220.0, 140.0))
        .unwrap();
    assert_eq!((moved.x, moved.y), (220.0, 140.0));
    app.build_frame().unwrap();
    let node = app.find_debug_selector("preview").unwrap();
    let rect = app.tree().rect(node).unwrap();
    assert_eq!((rect.x, rect.y), (220.0, 140.0));
    app.tree_mut().queue_interaction_event(
        node,
        aqua_ui::tree::interaction::InteractionEvent::Hover(true),
    );
    app.tree_mut().drain_commands();
    app.build_frame().unwrap();
    let rect = app.tree().rect(node).unwrap();
    assert_eq!((rect.x, rect.y), (220.0, 140.0));
    app.set_viewport(300.0, 180.0, 1.0);
    app.build_frame().unwrap();
    let rect = app.tree().rect(node).unwrap();
    assert_eq!((rect.x, rect.y), (220.0, 140.0));
    assert!(app.close_popup(popup));
    assert!(app
        .set_popup_pointer_anchor(popup, Point::new(30.0, 40.0))
        .is_none());
}
