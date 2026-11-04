use std::cell::Cell;
use std::rc::Rc;

use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::Canvas;
use aqua_ui::style::{px, Style};

#[test]
fn visibility_delivery_follows_emission_and_viewport_invalidation() {
    let observed = Rc::new(Cell::new(0));
    let delivered = Rc::new(Cell::new(0));
    let paint_count = observed.clone();
    let action_count = delivered.clone();
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(100.0, 100.0, 1.0);
    app.mount(|_| {
        Canvas::new()
            .style(Style::new().width(px(30.0)).height(px(30.0)))
            .on_paint_context(move |context| {
                paint_count.set(paint_count.get() + 1);
                if !context.bounds.intersect(context.clip).is_empty() {
                    let action_count = action_count.clone();
                    context.defer(move || action_count.set(action_count.get() + 1));
                }
                Vec::new()
            })
            .build()
    })
    .unwrap();
    assert_eq!(delivered.get(), 0);
    app.build_frame().unwrap();
    assert_eq!(observed.get(), 1);
    assert_eq!(delivered.get(), 1);
    app.build_frame().unwrap();
    assert_eq!(observed.get(), 1);
    assert_eq!(delivered.get(), 1);
    app.set_viewport(80.0, 80.0, 1.0);
    app.build_frame().unwrap();
    assert_eq!(observed.get(), 2);
    assert_eq!(delivered.get(), 2);
}
