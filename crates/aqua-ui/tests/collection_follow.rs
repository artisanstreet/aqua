use std::rc::Rc;

use aqua_core::{Point, Size};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::collection::{
    CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
    FollowMode, FollowState,
};
use aqua_ui::element::{Box, Element, Key};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::{px, Style};
use aqua_ui::tree::UiTree;

struct Rows {
    len: usize,
    height: f32,
}
impl CollectionSource for Rows {
    fn len(&self) -> usize {
        self.len
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
            .style(Style::new().height(px(self.height)).flex_shrink(0.0))
            .build()
    }
    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }
    fn validate(&self) -> Result<(), CollectionError> {
        Ok(())
    }
}
fn list(len: usize, height: f32) -> Element {
    Element::collection(CollectionDescriptor::new(
        Rc::new(Rows { len, height }),
        vec![Style::new().width(px(100.0)).height(px(100.0))],
        true,
        20.0,
        2,
    ))
    .with_key("list")
}
fn drive(view: &mut View, tree: &mut UiTree) {
    view.compute_layout(tree, Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
}

#[test]
fn tail_tracks_growth_pauses_and_reengages_only_after_layout() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let root = view.set_content(&mut tree, vec![list(20, 20.0)]).unwrap()[0];
    drive(&mut view, &mut tree);
    assert_eq!(
        view.collection_follow_state(root),
        Some(FollowState::Normal)
    );
    assert!(view.set_collection_follow_mode(&mut tree, root, FollowMode::Tail));
    drive(&mut view, &mut tree);
    assert_eq!(
        view.collection_follow_state(root),
        Some(FollowState::Tail { is_following: true })
    );
    assert!((tree.scroll_offset(root).unwrap().y - 300.0).abs() < 0.01);
    view.set_content(&mut tree, vec![list(25, 20.0)]).unwrap();
    drive(&mut view, &mut tree);
    assert!((tree.scroll_offset(root).unwrap().y - 400.0).abs() < 0.01);
    tree.scroll_to(root, Point::new(0.0, 100.0));
    drive(&mut view, &mut tree);
    assert_eq!(
        view.collection_follow_state(root),
        Some(FollowState::Tail {
            is_following: false
        })
    );
    view.set_content(&mut tree, vec![list(30, 20.0)]).unwrap();
    drive(&mut view, &mut tree);
    assert!((tree.scroll_offset(root).unwrap().y - 100.0).abs() < 0.01);
    tree.scroll_to(root, Point::new(0.0, 499.5));
    assert_eq!(
        view.collection_follow_state(root),
        Some(FollowState::Tail {
            is_following: false
        })
    );
    drive(&mut view, &mut tree);
    assert_eq!(
        view.collection_follow_state(root),
        Some(FollowState::Tail { is_following: true })
    );
    assert!(view.pause_collection_following_tail(&mut tree, root));
    assert_eq!(
        view.collection_follow_state(root),
        Some(FollowState::Tail {
            is_following: false
        })
    );
    view.set_content(&mut tree, vec![list(35, 20.0)]).unwrap();
    drive(&mut view, &mut tree);
    assert_eq!(
        view.collection_follow_state(root),
        Some(FollowState::Tail {
            is_following: false
        })
    );
    assert!(tree.scroll_offset(root).unwrap().y < 550.0);
    assert!(view.set_collection_follow_mode(&mut tree, root, FollowMode::Normal));
    view.set_content(&mut tree, vec![list(40, 20.0)]).unwrap();
    drive(&mut view, &mut tree);
    assert_eq!(
        view.collection_follow_state(root),
        Some(FollowState::Normal)
    );
    assert!(tree.scroll_offset(root).unwrap().y < 700.0);
}

#[test]
fn measured_height_growth_follows_tail_and_programmatic_reveal_pauses() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let root = view.set_content(&mut tree, vec![list(40, 20.0)]).unwrap()[0];
    drive(&mut view, &mut tree);
    assert!(view.set_collection_follow_mode(&mut tree, root, FollowMode::Tail));
    drive(&mut view, &mut tree);
    let before = tree.scroll_offset(root).unwrap().y;
    assert!(view.update_collection_item_height(&mut tree, "list", 0, 40.0));
    drive(&mut view, &mut tree);
    assert!((tree.scroll_offset(root).unwrap().y - before - 20.0).abs() < 0.01);
    assert!(view.reveal_collection_index(&mut tree, root, 0));
    assert_eq!(
        view.collection_follow_state(root),
        Some(FollowState::Tail {
            is_following: false
        })
    );
    drive(&mut view, &mut tree);
    assert!((tree.scroll_offset(root).unwrap().y).abs() < 0.01);
}
