use aqua_core::{InputEvent, Point, ScrollDelta};
use std::rc::Rc;

use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::collection::{
    CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
    FollowMode, FollowState,
};
use aqua_ui::element::{Box, Element, Key};
use aqua_ui::style::{px, Style};

struct Rows;
impl CollectionSource for Rows {
    fn len(&self) -> usize {
        20
    }
    fn key(&self, index: usize) -> Key {
        index.into()
    }
    fn layout(&self, width: f32) -> CollectionLayout {
        CollectionLayout {
            content_width: width,
            column_widths: Vec::new(),
        }
    }
    fn header(&self, _: &CollectionLayout) -> Option<Element> {
        None
    }
    fn row(&self, _: usize, _: &CollectionLayout) -> Element {
        Box::new()
            .style(Style::new().height(px(20.0)).flex_shrink(0.0))
            .build()
    }
    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }
    fn validate(&self) -> Result<(), CollectionError> {
        Ok(())
    }
}

#[test]
fn keyed_runtime_access_reports_real_follow_state() {
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(100.0, 100.0, 1.0);
    app.mount(|_| {
        Element::collection(CollectionDescriptor::new(
            Rc::new(Rows),
            vec![Style::new().width(px(100.0)).height(px(100.0))],
            true,
            20.0,
            2,
        ))
        .with_key("list")
    })
    .unwrap();
    app.build_frame().unwrap();
    assert_eq!(
        app.collection_follow_state("list"),
        Some(FollowState::Normal)
    );
    assert_eq!(app.collection_follow_state("missing"), None);
    assert!(app.set_collection_follow_mode("list", FollowMode::Tail));
    app.build_frame().unwrap();
    assert_eq!(
        app.collection_follow_state("list"),
        Some(FollowState::Tail { is_following: true })
    );
    let root = app.tree().root().unwrap();
    assert!(app.tree().scroll_offset(root).unwrap().y > 0.0);
    app.handle_input(&InputEvent::Scroll {
        at: Point::new(10.0, 10.0),
        delta: ScrollDelta { x: 0.0, y: -40.0 },
    });
    assert_eq!(
        app.collection_follow_state("list"),
        Some(FollowState::Tail {
            is_following: false
        })
    );
    assert!(app.set_collection_follow_mode("list", FollowMode::Tail));
    app.build_frame().unwrap();
    assert!(app.scroll_collection_to("list", Point::new(0.0, 20.0)));
    assert_eq!(
        app.collection_follow_state("list"),
        Some(FollowState::Tail {
            is_following: false
        })
    );
    assert!(app.set_collection_follow_mode("list", FollowMode::Tail));
    app.build_frame().unwrap();
    assert!(app.pause_collection_following_tail("list"));
    assert_eq!(
        app.collection_follow_state("list"),
        Some(FollowState::Tail {
            is_following: false
        })
    );
    assert!(!app.set_collection_follow_mode("missing", FollowMode::Tail));
}
