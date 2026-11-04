use aqua::{ui::Box as UiBox, UiApp};
use aqua_core::Rect;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    overlay::{Align, Placement, Side},
    popup::PopupOptions,
};

#[test]
fn measured_popup_can_reposition_without_clamping_and_switch_sides() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(200.0)
            .height(200.0)
            .child(
                UiBox::new()
                    .width(20.0)
                    .height(10.0)
                    .build()
                    .with_key("anchor")
                    .focusable(true),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(200.0, 200.0, 1.0);
    app.build_frame().unwrap();
    assert!(app.focus_key("anchor"));
    let owner = app.tree().focused().unwrap();
    let anchor = app.tree().visual_rect(owner).unwrap();
    let mut options = PopupOptions::tooltip();
    options.placement = Placement {
        preferred: Side::After,
        align: Align::Center,
        gap: 8.0,
        margin: 0.0,
        flip: false,
        centered: false,
    };
    let overlay = app
        .open_popup_measured(
            owner,
            UiBox::new().width(100.0).height(100.0).build(),
            options,
        )
        .unwrap();
    let contained = app.host().service().bounds(overlay).unwrap();
    assert!(contained.y >= 0.0);
    let uncontained = app.set_popup_uncontained(overlay, true).unwrap();
    assert_eq!(
        uncontained,
        Rect::from_edges(
            anchor.right() + 8.0,
            anchor.y + (anchor.h - 100.0) / 2.0,
            anchor.right() + 108.0,
            anchor.y + (anchor.h + 100.0) / 2.0
        )
    );
    let left = app
        .set_popup_placement(
            overlay,
            Placement {
                preferred: Side::Before,
                align: Align::Center,
                gap: 8.0,
                margin: 0.0,
                flip: false,
                centered: false,
            },
        )
        .unwrap();
    assert_eq!(left.x, anchor.x - 108.0);
    assert_eq!(left.y, uncontained.y);
    app.set_viewport(150.0, 150.0, 1.0);
    app.build_frame().unwrap();
    assert_eq!(app.host().service().bounds(overlay).unwrap().x, left.x);
    assert!(app.close_popup(overlay));
    assert!(app
        .set_popup_placement(overlay, Placement::below())
        .is_none());
}
