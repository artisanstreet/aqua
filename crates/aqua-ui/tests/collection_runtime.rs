//! View collection driver tests (driver owner).
//!
//! Drives `View::compute_layout` directly over `UiTree` (no `UiApp`): window
//! bounds, exact factory counts, keyed reuse, scope disposal, pinning,
//! anchors, measured settle, validation, header/plan discipline, and
//! semantics. A hand-rolled [`CollectionSource`] keeps factory-call
//! accounting exact; sources are test-local harnesses, not parallel
//! engines.

use std::cell::Cell;
use std::rc::Rc;

use aqua_core::NodeId;
use aqua_core::{Point, Role, Size};
use aqua_reactive::{ReadSignal, Runtime, WriteSignal};
use aqua_ui::binding::View;
use aqua_ui::collection_runtime::{
    CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
};
use aqua_ui::element::{Element, Key, ui};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::tree::{ActionEvent, ActionKind, NodeKind, UiTree};

// ---------------------------------------------------------------------------
// Harness source (counting, variable heights, optional header)
// ---------------------------------------------------------------------------

struct TItem {
    key: u64,
    label: String,
    height: f32,
}

struct TSource {
    items: Vec<TItem>,
    row_calls: Cell<usize>,
    header_calls: Cell<usize>,
    with_header: bool,
}

impl TSource {
    fn plain(n: u64) -> Self {
        Self {
            items: (0..n)
                .map(|i| TItem {
                    key: i,
                    label: format!("row-{i}"),
                    height: 32.0,
                })
                .collect(),
            row_calls: Cell::new(0),
            header_calls: Cell::new(0),
            with_header: false,
        }
    }

    fn varied(n: u64) -> Self {
        Self {
            items: (0..n)
                .map(|i| TItem {
                    key: i,
                    label: format!("row-{i}"),
                    height: 24.0 + ((i % 5) as f32) * 8.0,
                })
                .collect(),
            row_calls: Cell::new(0),
            header_calls: Cell::new(0),
            with_header: false,
        }
    }
}

impl CollectionSource for TSource {
    fn len(&self) -> usize {
        self.items.len()
    }

    fn key(&self, index: usize) -> Key {
        self.items[index].key.into()
    }

    fn layout(&self, available_width: f32) -> CollectionLayout {
        CollectionLayout {
            content_width: available_width.max(0.0),
            column_widths: Vec::new(),
        }
    }

    fn row(&self, index: usize, _layout: &CollectionLayout) -> Element {
        self.row_calls.set(self.row_calls.get() + 1);
        let it = &self.items[index];
        ui::Box::builder()
            .height(it.height)
            .child(ui::Text::builder().arg(it.label.clone()).build())
            .build()
            .role(Role::ListItem)
    }

    fn header(&self, _layout: &CollectionLayout) -> Option<Element> {
        if !self.with_header {
            return None;
        }
        self.header_calls.set(self.header_calls.get() + 1);
        Some(ui::Text::builder().arg("hdr").build())
    }

    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }

    fn validate(&self) -> Result<(), CollectionError> {
        let mut seen = std::collections::HashSet::new();
        for it in &self.items {
            if !seen.insert(it.key) {
                return Err(CollectionError::DuplicateKey);
            }
        }
        Ok(())
    }
}

fn desc_of(source: &Rc<TSource>, virtualized: bool) -> CollectionDescriptor {
    CollectionDescriptor::new(
        Rc::clone(source) as Rc<dyn CollectionSource>,
        Vec::new(),
        virtualized,
        32.0,
        4,
    )
}

fn mount_new(tree: &mut UiTree, virtualized: bool, source: &Rc<TSource>) -> (Runtime, View) {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let mut view = View::new(scope, tree.command_sink());
    view.set_content(
        tree,
        vec![Element::collection(desc_of(source, virtualized))],
    )
    .expect("mount collects");
    (rt, view)
}

fn drive(view: &mut View, tree: &mut UiTree, w: f32, h: f32) {
    view.compute_layout(tree, Size::new(w, h), &NaiveMeasurer)
        .expect("frame drives");
}

fn row_texts(tree: &UiTree) -> Vec<String> {
    tree.traverse()
        .into_iter()
        .filter_map(|n| match tree.get(n).map(|r| &r.kind) {
            Some(NodeKind::Text(t)) if !t.is_editable() && !t.text().is_empty() => {
                let s = t.text().to_string();
                s.starts_with("row-").then_some(s)
            }
            _ => None,
        })
        .collect()
}

fn text_node(tree: &UiTree, content: &str) -> Option<NodeId> {
    tree.traverse().into_iter().find(
        |n| matches!(tree.get(*n).map(|r| &r.kind), Some(NodeKind::Text(t)) if t.text() == content),
    )
}

#[test]
fn width_probe_measures_offscreen_row_and_updates_scroll_extent() {
    let mut source = TSource::plain(1000);
    let widest = "wide".repeat(80);
    source.items[999].label = widest.clone();
    let source = Rc::new(source);
    let mut descriptor = desc_of(&source, true);
    descriptor.width_probe = Some(999);
    let mut tree = UiTree::new();
    let runtime = Runtime::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![Element::collection(descriptor)])
        .unwrap();
    drive(&mut view, &mut tree, 200.0, 160.0);
    let root = view.root().unwrap();
    assert!(tree.content_size(root).unwrap().w > 2000.0);
    assert!(text_node(&tree, &widest).is_none());
    assert!(source.row_calls.get() < 50);
    assert!(tree.scroll_to(root, Point::new(800.0, 0.0)));
    assert!(tree.scroll_offset(root).unwrap().x > 0.0);

    struct DoubleWidth;
    impl aqua_ui::layout::TextMeasurer for DoubleWidth {
        fn measure_text(
            &self,
            text: &str,
            font_size: f32,
            max_width: Option<f32>,
            multiline: bool,
        ) -> Size {
            let size = aqua_ui::layout::TextMeasurer::measure_text(
                &NaiveMeasurer,
                text,
                font_size,
                max_width,
                multiline,
            );
            Size::new(size.w * 2.0, size.h)
        }
    }
    let width = tree.content_size(root).unwrap().w;
    view.compute_layout(&mut tree, Size::new(200.0, 160.0), &DoubleWidth)
        .unwrap();
    assert!(tree.content_size(root).unwrap().w > width * 1.5);

    let replacement = Rc::new(TSource::plain(1000));
    let mut descriptor = desc_of(&replacement, true);
    descriptor.width_probe = Some(999);
    view.set_content(&mut tree, vec![Element::collection(descriptor)])
        .unwrap();
    drive(&mut view, &mut tree, 200.0, 160.0);
    assert!(tree.content_size(root).unwrap().w < 2000.0);
}

#[test]
fn prepared_rows_fail_before_registration_and_remain_bounded_on_retry() {
    use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
    use aqua_ui::collection::{List, PreparedCollectionRow};
    use std::sync::Arc;

    let failing = Rc::new(Cell::new(true));
    let calls = Rc::new(Cell::new(0));
    let factory_failure = Rc::clone(&failing);
    let factory_calls = Rc::clone(&calls);
    let descriptor = List::new()
        .items(0..1000usize)
        .key(|index| *index)
        .try_row(move |index| {
            factory_calls.set(factory_calls.get() + 1);
            if *index == 1 && factory_failure.get() {
                return Err(CollectionError::Validation("unsupported row".into()));
            }
            Ok(PreparedCollectionRow {
                element: ui::Text::builder().arg(format!("row-{index}")).build(),
                selection_sources: vec![(
                    SelectionDocumentId(3),
                    SelectionSource {
                        id: SelectionSourceId(*index as u64),
                        order: vec![*index as u64],
                        text: Arc::from(format!("complete body {index}")),
                        separator: Arc::from("\n"),
                        copies: Vec::new(),
                    },
                )],
            })
        })
        .virtualize()
        .descriptor();
    let mut tree = UiTree::new();
    let runtime = Runtime::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![Element::collection(descriptor)])
        .unwrap();
    let registrations = Rc::new(Cell::new(0));
    let registered = Rc::clone(&registrations);
    let mut register = move |sources: Vec<(SelectionDocumentId, SelectionSource)>| {
        registered.set(registered.get() + sources.len());
        Ok(())
    };
    assert!(
        view.compute_layout_with_row_sources(
            &mut tree,
            Size::new(200.0, 160.0),
            &NaiveMeasurer,
            &mut register
        )
        .is_err()
    );
    assert_eq!(registrations.get(), 0);
    assert!(row_texts(&tree).is_empty());
    failing.set(false);
    view.compute_layout_with_row_sources(
        &mut tree,
        Size::new(200.0, 160.0),
        &NaiveMeasurer,
        &mut register,
    )
    .unwrap();
    assert!(registrations.get() > 1);
    assert!(calls.get() < 50);
    let prior = registrations.get();
    view.compute_layout_with_row_sources(
        &mut tree,
        Size::new(200.0, 160.0),
        &NaiveMeasurer,
        &mut register,
    )
    .unwrap();
    assert_eq!(registrations.get(), prior);
    let root = view.root().unwrap();
    assert!(tree.scroll_to(root, Point::new(0.0, 8000.0)));
    view.compute_layout_with_row_sources(
        &mut tree,
        Size::new(200.0, 160.0),
        &NaiveMeasurer,
        &mut register,
    )
    .unwrap();
    assert!(registrations.get() > prior);
    assert!(calls.get() < 100);
    let before_refresh = registrations.get();
    let replacement = List::new()
        .items(0..1000usize)
        .key(|index| *index)
        .try_row(|index| {
            Ok(PreparedCollectionRow {
                element: ui::Text::builder().arg(format!("new-{index}")).build(),
                selection_sources: vec![(
                    SelectionDocumentId(3),
                    SelectionSource {
                        id: SelectionSourceId(*index as u64),
                        order: vec![*index as u64],
                        text: Arc::from(format!("new complete body {index}")),
                        separator: Arc::from("\n"),
                        copies: Vec::new(),
                    },
                )],
            })
        })
        .virtualize()
        .descriptor();
    view.set_content(&mut tree, vec![Element::collection(replacement)])
        .unwrap();
    view.compute_layout_with_row_sources(
        &mut tree,
        Size::new(200.0, 160.0),
        &NaiveMeasurer,
        &mut register,
    )
    .unwrap();
    assert!(registrations.get() > before_refresh);
}

#[test]
fn fallible_list_width_probe_uses_only_inert_factory() {
    use aqua_ui::collection::{List, PreparedCollectionRow};

    let probed = Rc::new(Cell::new(0));
    let prepared_probe = Rc::new(Cell::new(false));
    let probe_calls = Rc::clone(&probed);
    let prepared = Rc::clone(&prepared_probe);
    let descriptor = List::new()
        .items(0..1000usize)
        .key(|index| *index)
        .row(move |index| {
            if *index == 999 {
                probe_calls.set(probe_calls.get() + 1);
            }
            ui::Text::builder().arg("wide".repeat(80)).build()
        })
        .try_row(move |index| {
            if *index == 999 {
                prepared.set(true);
            }
            Ok(PreparedCollectionRow {
                element: ui::Text::builder().arg(format!("row-{index}")).build(),
                selection_sources: Vec::new(),
            })
        })
        .width_probe(999)
        .virtualize()
        .descriptor();
    let mut tree = UiTree::new();
    let runtime = Runtime::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![Element::collection(descriptor)])
        .unwrap();
    drive(&mut view, &mut tree, 200.0, 160.0);
    assert!(probed.get() > 0);
    assert!(!prepared_probe.get());
    assert!(tree.content_size(view.root().unwrap()).unwrap().w > 2000.0);
}

#[test]
fn width_sensitive_rows_use_committed_inner_width_on_first_frame_and_resize() {
    use aqua_core::Insets;
    use aqua_ui::collection::{BatchedCollectionRow, List};
    use aqua_ui::style::{Length, Style};
    use std::cell::RefCell;

    let measured = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&measured);
    let descriptor = List::new()
        .items(0..1000usize)
        .key(|index| *index)
        .try_batched_row_with_layout(move |index, layout| {
            seen.borrow_mut().push((*index, layout.content_width));
            Ok(BatchedCollectionRow {
                element: ui::Text::builder().arg(format!("row-{index}")).build(),
                batches: Vec::new(),
            })
        })
        .style(
            Style::new()
                .width(Length::Pct(50.0))
                .height(Length::Px(160.0))
                .padding_insets(Insets {
                    left: 20.0,
                    right: 30.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .virtualize()
        .descriptor();
    let mut tree = UiTree::new();
    let runtime = Runtime::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![Element::collection(descriptor)])
        .unwrap();
    assert!(measured.borrow().is_empty());
    drive(&mut view, &mut tree, 1000.0, 400.0);
    let initial = measured.borrow().clone();
    assert!(!initial.is_empty());
    assert!(initial.len() < 50);
    assert!(initial.iter().all(|(_, width)| *width == 450.0));
    drive(&mut view, &mut tree, 1000.0, 400.0);
    assert_eq!(measured.borrow().len(), initial.len());
    drive(&mut view, &mut tree, 800.0, 400.0);
    let next = measured.borrow();
    let resized = &next[initial.len()..];
    assert!(!resized.is_empty());
    assert!(resized.len() < 50);
    assert!(resized.iter().all(|(_, width)| *width == 350.0));
}

#[test]
fn width_class_keeps_rows_while_the_class_is_stable_and_rebuilds_when_it_changes() {
    use aqua_ui::collection::{BatchedCollectionRow, List};
    use aqua_ui::style::{Length, Style};
    use std::cell::RefCell;

    let measured = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&measured);
    let descriptor = List::new()
        .items(0..1000usize)
        .key(|index| *index)
        .width_class(|width| width.min(500.0))
        .try_batched_row_with_layout(move |index, layout| {
            seen.borrow_mut().push((*index, layout.content_width));
            Ok(BatchedCollectionRow {
                element: ui::Text::builder().arg(format!("row-{index}")).build(),
                batches: Vec::new(),
            })
        })
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Px(160.0)),
        )
        .virtualize()
        .descriptor();
    let mut tree = UiTree::new();
    let runtime = Runtime::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![Element::collection(descriptor)])
        .unwrap();
    drive(&mut view, &mut tree, 900.0, 400.0);
    let initial = measured.borrow().len();
    assert!(initial > 0);
    drive(&mut view, &mut tree, 700.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    assert_eq!(measured.borrow().len(), initial);
    drive(&mut view, &mut tree, 400.0, 400.0);
    let next = measured.borrow();
    let rebuilt = &next[initial..];
    assert!(!rebuilt.is_empty());
    assert!(rebuilt.iter().all(|(_, width)| *width == 400.0));
}

#[test]
fn width_sensitive_rows_follow_container_width_changes_at_a_fixed_viewport() {
    use aqua_core::Insets;
    use aqua_ui::collection::{BatchedCollectionRow, List};
    use aqua_ui::style::{Length, Style};
    use std::cell::RefCell;

    let measured = Rc::new(RefCell::new(Vec::new()));
    let descriptor = |percent: f32| {
        let seen = Rc::clone(&measured);
        List::new()
            .items(0..1000usize)
            .key(|index| *index)
            .try_batched_row_with_layout(move |index, layout| {
                seen.borrow_mut().push((*index, layout.content_width));
                Ok(BatchedCollectionRow {
                    element: ui::Text::builder().arg(format!("row-{index}")).build(),
                    batches: Vec::new(),
                })
            })
            .style(
                Style::new()
                    .width(Length::Pct(percent))
                    .height(Length::Px(160.0))
                    .padding_insets(Insets {
                        left: 20.0,
                        right: 30.0,
                        top: 0.0,
                        bottom: 0.0,
                    }),
            )
            .virtualize()
            .descriptor()
    };
    let mut tree = UiTree::new();
    let runtime = Runtime::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![Element::collection(descriptor(50.0))])
        .unwrap();
    drive(&mut view, &mut tree, 1000.0, 400.0);
    drive(&mut view, &mut tree, 1000.0, 400.0);
    let before = measured.borrow().len();
    view.set_content(&mut tree, vec![Element::collection(descriptor(25.0))])
        .unwrap();
    drive(&mut view, &mut tree, 1000.0, 400.0);
    let after = measured.borrow();
    let rebuilt = &after[before..];
    assert!(!rebuilt.is_empty());
    assert!(rebuilt.iter().all(|(_, width)| *width == 200.0));
}

/// Static texts under `id` (descendant walk): roles sit on row wrapper
/// nodes, readable text on their children.
fn descendant_texts(tree: &UiTree, id: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![id];
    while let Some(n) = stack.pop() {
        if let Some(rec) = tree.get(n) {
            if let NodeKind::Text(t) = &rec.kind {
                if !t.is_editable() && !t.text().is_empty() {
                    out.push(t.text().to_string());
                }
            }
            stack.extend(rec.children.iter().copied());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 1. Unvirtualized mounts every row through factories exactly once.
// ---------------------------------------------------------------------------

#[test]
fn unvirtualized_mounts_every_row_exactly_once() {
    let source = Rc::new(TSource::plain(32));
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, false, &source);
    drive(&mut view, &mut tree, 600.0, 800.0);
    drive(&mut view, &mut tree, 600.0, 800.0);
    assert_eq!(row_texts(&tree).len(), 32);
    assert_eq!(
        source.row_calls.get(),
        32,
        "one factory call per row, never per frame"
    );
}

// ---------------------------------------------------------------------------
// 2. Virtualized 100k bounds mounted rows and factory calls.
// ---------------------------------------------------------------------------

#[test]
fn virtualized_100k_stays_bounded() {
    let source = Rc::new(TSource::plain(100_000));
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, true, &source);
    drive(&mut view, &mut tree, 600.0, 600.0);
    drive(&mut view, &mut tree, 600.0, 600.0);
    let rows = row_texts(&tree);
    assert!(
        rows.len() < 400,
        "visible+overscan only, got {}",
        rows.len()
    );
    assert!(
        source.row_calls.get() < 400,
        "factories per mounted row only, got {}",
        source.row_calls.get()
    );
    assert!(tree.len() < 3000, "retained tree bounded: {}", tree.len());
    assert!(rows.iter().any(|t| t == "row-0"), "window starts at top");
}

// ---------------------------------------------------------------------------
// 3. Clean frames make zero factory calls.
// ---------------------------------------------------------------------------

#[test]
fn clean_frames_make_no_factory_calls() {
    let source = Rc::new(TSource::plain(5000));
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, true, &source);
    drive(&mut view, &mut tree, 600.0, 600.0);
    drive(&mut view, &mut tree, 600.0, 600.0);
    let settled = source.row_calls.get();
    drive(&mut view, &mut tree, 600.0, 600.0);
    drive(&mut view, &mut tree, 600.0, 600.0);
    assert_eq!(
        source.row_calls.get(),
        settled,
        "steady state runs zero factories"
    );
}

// ---------------------------------------------------------------------------
// 4. Scroll moves the window with O(window) factory work.
// ---------------------------------------------------------------------------

#[test]
fn scroll_moves_window_with_bounded_work() {
    let source = Rc::new(TSource::plain(20_000));
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, true, &source);
    drive(&mut view, &mut tree, 600.0, 600.0);
    drive(&mut view, &mut tree, 600.0, 600.0);
    let root = view.root().unwrap();
    assert!(tree.scroll_to(root, Point::new(0.0, 200_000.0)));
    drive(&mut view, &mut tree, 600.0, 600.0);
    drive(&mut view, &mut tree, 600.0, 600.0);
    let labels = row_texts(&tree);
    let max: usize = labels
        .iter()
        .filter_map(|t| t.strip_prefix("row-")?.parse().ok())
        .max()
        .unwrap();
    assert!(max > 5000, "window followed the scroll, max {max}");
    assert!(
        !labels.iter().any(|t| t == "row-0"),
        "head rows unmounted after scroll"
    );
    let before = source.row_calls.get();
    assert!(tree.scroll_to(root, Point::new(0.0, 400_000.0)));
    drive(&mut view, &mut tree, 600.0, 600.0);
    assert!(
        source.row_calls.get() - before < 500,
        "one scroll realizes O(window)"
    );
}

// ---------------------------------------------------------------------------
// 5. Live subscriptions survive window updates; eviction disposes;
// ghost writes queue nothing. Per-row signals (mirrors the strict
// conformance lifecycle at View level).
// ---------------------------------------------------------------------------

struct LiveSource {
    sigs: Rc<Vec<SigPair>>,
}

impl CollectionSource for LiveSource {
    fn len(&self) -> usize {
        self.sigs.len()
    }
    fn key(&self, index: usize) -> Key {
        (index as u64).into()
    }
    fn layout(&self, w: f32) -> CollectionLayout {
        CollectionLayout {
            content_width: w.max(0.0),
            column_widths: Vec::new(),
        }
    }
    fn row(&self, index: usize, _layout: &CollectionLayout) -> Element {
        ui::Text::builder()
            .value(self.sigs[index].0.clone())
            .build()
    }
    fn header(&self, _layout: &CollectionLayout) -> Option<Element> {
        None
    }
    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }
    fn validate(&self) -> Result<(), CollectionError> {
        Ok(())
    }
}

fn static_texts(tree: &UiTree) -> Vec<String> {
    tree.traverse()
        .into_iter()
        .filter_map(|n| match tree.get(n).map(|r| &r.kind) {
            Some(NodeKind::Text(t)) if !t.is_editable() && !t.text().is_empty() => {
                Some(t.text().to_string())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn unmounted_rows_dispose_scopes() {
    const N: usize = 500;
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let sigs: Rc<Vec<SigPair>> =
        Rc::new((0..N).map(|i| scope.signal(format!("sig-{i}"))).collect());
    let writers: Vec<WriteSignal<String>> = sigs.iter().map(|(_, w)| w.clone()).collect();
    let source: Rc<dyn CollectionSource> = Rc::new(LiveSource { sigs });
    let mut tree = UiTree::new();
    let scope2 = rt.create_scope();
    let mut view = View::new(scope2, tree.command_sink());
    view.set_content(
        &mut tree,
        vec![Element::collection(CollectionDescriptor::new(
            source,
            Vec::new(),
            true,
            32.0,
            4,
        ))],
    )
    .unwrap();
    drive(&mut view, &mut tree, 600.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    assert!(static_texts(&tree).iter().any(|t| t == "sig-0"));
    let stats0 = rt.stats();
    assert!(stats0.effects > 0, "resident rows hold live effects");
    // Small scroll retaining the first key: subscription stays live.
    let root = view.root().unwrap();
    assert!(tree.scroll_to(root, Point::new(0.0, 32.0)));
    drive(&mut view, &mut tree, 600.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    assert!(
        static_texts(&tree).iter().any(|t| t == "sig-0"),
        "small scroll retains the first key"
    );
    writers[0].set("sig-0-live".to_string());
    rt.flush();
    drive(&mut view, &mut tree, 600.0, 400.0);
    assert!(
        static_texts(&tree).iter().any(|t| t == "sig-0-live"),
        "signal write updates mounted text"
    );
    // Scroll far: first row evicted, resident effect budget bounded.
    assert!(tree.scroll_to(root, Point::new(0.0, 10_000_000.0)));
    drive(&mut view, &mut tree, 600.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    assert!(
        !static_texts(&tree).iter().any(|t| t.starts_with("sig-0")),
        "first key evicted from the far window"
    );
    assert!(
        rt.stats().effects < 600,
        "resident effect budget bounded: {}",
        rt.stats().effects
    );
    // Ghost write to the evicted signal queues nothing and remounts nothing.
    writers[0].set("sig-0-ghost".to_string());
    rt.flush();
    assert_eq!(
        rt.stats().queued_effects,
        0,
        "no effect queued for the evicted row"
    );
    drive(&mut view, &mut tree, 600.0, 400.0);
    assert!(
        !static_texts(&tree).iter().any(|t| t.starts_with("sig-0")),
        "evicted row stays unmounted after its signal fires"
    );
}

// ---------------------------------------------------------------------------
// 6. Focused far row pins in isolation (gap never mounts).
// ---------------------------------------------------------------------------

#[test]
fn focused_far_row_pins_alone() {
    let source = Rc::new(TSource::plain(20_000));
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, true, &source);
    drive(&mut view, &mut tree, 600.0, 600.0);
    drive(&mut view, &mut tree, 600.0, 600.0);
    let root = view.root().unwrap();
    assert!(tree.scroll_to(root, Point::new(0.0, 200_000.0)));
    drive(&mut view, &mut tree, 600.0, 600.0);
    drive(&mut view, &mut tree, 600.0, 600.0);
    let labels = row_texts(&tree);
    let pinned_label = labels[labels.len() / 2].clone();
    let pinned = text_node(&tree, &pinned_label).expect("row mounted");
    assert!(tree.set_focusable(pinned, true));
    assert!(tree.set_focused(Some(pinned)));
    assert!(tree.scroll_to(root, Point::new(0.0, 600_000.0)));
    drive(&mut view, &mut tree, 600.0, 600.0);
    drive(&mut view, &mut tree, 600.0, 600.0);
    assert!(tree.get(pinned).is_some(), "pinned row stays mounted");
    assert_eq!(tree.focused(), Some(pinned));
    assert!(
        row_texts(&tree).len() < 400,
        "window + isolated pin only, not the gap"
    );
}

// ---------------------------------------------------------------------------
// 7. Prepend preserves the reading anchor (stable key + within-row offset).
// ---------------------------------------------------------------------------

#[test]
fn prepend_preserves_reading_anchor() {
    let source = Rc::new(TSource::plain(2000));
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, true, &source);
    drive(&mut view, &mut tree, 600.0, 400.0);
    let root = view.root().unwrap();
    assert!(tree.scroll_to(root, Point::new(0.0, 30_000.0)));
    drive(&mut view, &mut tree, 600.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    /// Visual y (screen space): committed rect minus ancestor scroll.
    /// Prepend shifts committed positions down by the inserted height while
    /// the scroll follows; only the visual position is stable by design.
    fn visual_y(tree: &UiTree, id: aqua_core::NodeId) -> Option<f32> {
        let mut y = tree.rect(id)?.y;
        let mut next = tree.get(id)?.parent;
        while let Some(pid) = next {
            let rec = tree.get(pid)?;
            if let NodeKind::Scroll(s) = &rec.kind {
                y -= s.offset.y;
            }
            next = rec.parent;
        }
        Some(y)
    }
    let mut visible: Vec<(f32, String)> = row_texts(&tree)
        .into_iter()
        .filter_map(|t| {
            let id = text_node(&tree, &t)?;
            Some((visual_y(&tree, id)?, t))
        })
        .collect();
    visible.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert!(visible.len() > 4);
    let (y0, anchor) = visible[visible.len() / 2].clone();
    let next = Rc::new(TSource {
        items: (0..20u64)
            .map(|j| TItem {
                key: 2_000_000 + j,
                label: format!("row-{}", 2_000_000 + j),
                height: 32.0,
            })
            .chain((0..2000u64).map(|i| TItem {
                key: i,
                label: format!("row-{i}"),
                height: 32.0,
            }))
            .collect(),
        row_calls: Cell::new(0),
        header_calls: Cell::new(0),
        with_header: false,
    });
    view.set_content(&mut tree, vec![Element::collection(desc_of(&next, true))])
        .unwrap();
    drive(&mut view, &mut tree, 600.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    let id = text_node(&tree, &anchor).expect("anchor still mounted");
    let y1 = visual_y(&tree, id).expect("anchor visual rect");
    assert!(
        (y1 - y0).abs() <= 48.0,
        "anchor preserved: {anchor} was {y0}, now {y1}"
    );
}

// ---------------------------------------------------------------------------
// 8. Measured heights settle boundedly; extra drives stay quiet.
// ---------------------------------------------------------------------------

#[test]
fn measured_heights_settle_and_stay_quiet() {
    let rt = Runtime::new();
    let source = Rc::new(TSource::varied(3000));
    let mut tree = UiTree::new();
    let scope = rt.create_scope();
    let mut view = View::new(scope, tree.command_sink());
    view.set_content(&mut tree, vec![Element::collection(desc_of(&source, true))])
        .unwrap();
    drive(&mut view, &mut tree, 600.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    let calls = source.row_calls.get();
    drive(&mut view, &mut tree, 600.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    assert_eq!(source.row_calls.get(), calls, "settled: no new factories");
    let root = view.root().unwrap();
    let before = source.row_calls.get();
    assert!(tree.scroll_to(root, Point::new(0.0, 60_000.0)));
    drive(&mut view, &mut tree, 600.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    assert!(
        source.row_calls.get() - before < 500,
        "bounded realization into varied heights"
    );
    let quiet = source.row_calls.get();
    drive(&mut view, &mut tree, 600.0, 400.0);
    assert_eq!(source.row_calls.get(), quiet, "converged: quiet again");
}

// ---------------------------------------------------------------------------
// 9. Validation surfaces loudly (mount and refresh).
// ---------------------------------------------------------------------------

#[test]
fn duplicate_keys_fail_mount() {
    let source = Rc::new(TSource {
        items: vec![
            TItem {
                key: 1,
                label: "row-1".into(),
                height: 32.0,
            },
            TItem {
                key: 1,
                label: "row-dup".into(),
                height: 32.0,
            },
        ],
        row_calls: Cell::new(0),
        header_calls: Cell::new(0),
        with_header: false,
    });
    let mut tree = UiTree::new();
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let mut view = View::new(scope, tree.command_sink());
    let res = view.set_content(
        &mut tree,
        vec![Element::collection(desc_of(&source, false))],
    );
    assert!(res.is_err(), "duplicate keys fail at mount, never silent");
}

#[test]
fn refresh_validation_failure_surfaces() {
    let source = Rc::new(TSource::plain(8));
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, false, &source);
    drive(&mut view, &mut tree, 600.0, 600.0);
    let bad = Rc::new(TSource {
        items: vec![
            TItem {
                key: 7,
                label: "row-7".into(),
                height: 32.0,
            },
            TItem {
                key: 7,
                label: "row-dup".into(),
                height: 32.0,
            },
        ],
        row_calls: Cell::new(0),
        header_calls: Cell::new(0),
        with_header: false,
    });
    let res = view.set_content(&mut tree, vec![Element::collection(desc_of(&bad, false))]);
    assert!(res.is_err(), "refresh validation fails loudly");
}

#[test]
fn invalid_estimated_height_fails_mount() {
    let source = Rc::new(TSource::plain(4));
    let mut tree = UiTree::new();
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let mut view = View::new(scope, tree.command_sink());
    let desc = CollectionDescriptor::new(
        Rc::clone(&source) as Rc<dyn CollectionSource>,
        Vec::new(),
        true,
        f32::NAN,
        4,
    );
    let res = view.set_content(&mut tree, vec![Element::collection(desc)]);
    assert!(res.is_err(), "non-finite estimate fails at mount");
}

// ---------------------------------------------------------------------------
// 10. List width change rebuilds nothing (column_widths identical).
// ---------------------------------------------------------------------------

#[test]
fn list_width_change_rebuilds_nothing() {
    // Lists have no headers: the source header factory never runs.
    let source = Rc::new(TSource {
        with_header: true,
        ..TSource::plain(2000)
    });
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, true, &source);
    drive(&mut view, &mut tree, 600.0, 600.0);
    drive(&mut view, &mut tree, 600.0, 600.0);
    assert_eq!(source.header_calls.get(), 0, "lists build no headers");
    let rows = source.row_calls.get();
    drive(&mut view, &mut tree, 600.0, 600.0);
    assert_eq!(source.row_calls.get(), rows);
    // Lists echo width into content_width only: width change must NOT
    // rebuild rows (column_widths identical).
    drive(&mut view, &mut tree, 900.0, 600.0);
    drive(&mut view, &mut tree, 900.0, 600.0);
    assert_eq!(
        source.row_calls.get(),
        rows,
        "list width change rebuilds nothing"
    );
}

// ---------------------------------------------------------------------------
// 10b. Tables: first-frame vertical stacking, header-once, roles.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct TCell {
    key: u64,
    name: String,
    value: u64,
}

fn table_element(
    items: Vec<TCell>,
    cell_calls: Rc<Cell<usize>>,
    header_calls: Rc<Cell<usize>>,
    virtualized: bool,
) -> Element {
    use aqua_ui::collection::{ColumnBuilder, Table};
    use aqua_ui::style::Length;
    let cc = cell_calls.clone();
    let hc = header_calls.clone();
    let mut t = Table::new()
        .items(items)
        .key(|it: &TCell| it.key)
        .columns([
            ColumnBuilder::new("name")
                .header(move || {
                    hc.set(hc.get() + 1);
                    ui::Text::builder().arg("Name").build()
                })
                .cell(move |it: &TCell| {
                    cc.set(cc.get() + 1);
                    ui::Text::builder().arg(it.name.clone()).build()
                })
                .width(Length::Px(400.0))
                .build(),
            ColumnBuilder::new("value")
                .header(|| ui::Text::builder().arg("Value").build())
                .cell(|it: &TCell| ui::Text::builder().arg(format!("{}", it.value)).build())
                .width(Length::Px(400.0))
                .build(),
        ])
        .header_styles([aqua_ui::style::Style::new()])
        .row_styles([aqua_ui::style::Style::new()]);
    if virtualized {
        t = t.virtualize();
    }
    t.build()
}

fn cell_rects(tree: &UiTree, label: &str) -> Vec<(f32, f32, f32, f32)> {
    tree.traverse()
        .into_iter()
        .filter_map(|n| match tree.get(n).map(|r| &r.kind) {
            Some(NodeKind::Text(t)) if t.text() == label => {
                tree.rect(n).map(|r| (r.x, r.y, r.w, r.h))
            }
            _ => None,
        })
        .collect()
}

/// Visual (screen-space) rect: committed layout rect minus every ancestor
/// `Scroll` offset. Layout rects ignore scroll by design (scene emission
/// and hit-testing apply the translation), so scroll assertions and press
/// positions must go through here, never raw `rect()`.
fn visual_rect_of(tree: &UiTree, id: NodeId) -> Option<(f32, f32, f32, f32)> {
    let mut rect = tree.rect(id)?;
    let mut next = tree.get(id)?.parent;
    while let Some(pid) = next {
        let rec = tree.get(pid)?;
        if let NodeKind::Scroll(s) = &rec.kind {
            rect.x -= s.offset.x;
            rect.y -= s.offset.y;
        }
        next = rec.parent;
    }
    Some((rect.x, rect.y, rect.w, rect.h))
}

fn visual_x_of(tree: &UiTree, label: &str) -> f32 {
    let id = tree
        .traverse()
        .into_iter()
        .find(|n| {
            matches!(tree.get(*n).map(|r| &r.kind), Some(NodeKind::Text(t)) if t.text() == label)
        })
        .expect("cell mounted");
    visual_rect_of(tree, id).expect("cell rect").0
}

/// Header visibility regression (stretched/invisible-header artifact):
/// committed visual header height > 0, header bottom at/above first body
/// top, no overlap. Label presence alone proves nothing.
fn assert_header_visible_at(tree: &UiTree, header_label: &str, first_body_label: &str) {
    let hid = tree
        .traverse()
        .into_iter()
        .find(|n| {
            matches!(tree.get(*n).map(|r| &r.kind), Some(NodeKind::Text(t)) if t.text() == header_label)
        })
        .expect("header mounted");
    let bid = tree
        .traverse()
        .into_iter()
        .find(|n| {
            matches!(tree.get(*n).map(|r| &r.kind), Some(NodeKind::Text(t)) if t.text() == first_body_label)
        })
        .expect("first body row mounted");
    let (hx, hy, _hw, hh) = visual_rect_of(tree, hid).expect("header rect");
    let (bx, by, _bw, _bh) = visual_rect_of(tree, bid).expect("body rect");
    assert!(hh > 0.0, "committed visual header height > 0");
    assert!(
        hy + hh <= by + 1.0,
        "header bottom at/above first body top: ({hx},{hy}+{hh}) vs ({bx},{by})"
    );
}

#[test]
fn table_first_frame_stacks_rows_vertically() {
    let cell_calls = Rc::new(Cell::new(0));
    let header_calls = Rc::new(Cell::new(0));
    let items: Vec<TCell> = (0..8u64)
        .map(|i| TCell {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let mut tree = UiTree::new();
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let mut view = View::new(scope, tree.command_sink());
    view.set_content(
        &mut tree,
        vec![table_element(
            items,
            cell_calls.clone(),
            header_calls.clone(),
            false,
        )],
    )
    .unwrap();
    drive(&mut view, &mut tree, 700.0, 600.0);
    drive(&mut view, &mut tree, 700.0, 600.0);
    // Header above the first body row; body rows stack vertically with
    // increasing y and sane heights (never stretched to the body height).
    let name_hx = cell_rects(&tree, "Name");
    assert_eq!(name_hx.len(), 1, "one Name header cell, got {name_hx:?}");
    let mut prev_bottom = name_hx[0].1 + name_hx[0].3;
    for i in 0..8 {
        let r = cell_rects(&tree, &format!("name-{i}"));
        assert_eq!(r.len(), 1, "one name-{i} cell, got {r:?}");
        let (x, y, _w, h) = r[0];
        assert!(
            y >= prev_bottom - 1.0,
            "row {i} stacks below the header/previous row"
        );
        assert!(
            h > 0.0 && h < 600.0,
            "row {i} has a sane height, not body-stretched ({h})"
        );
        assert!(
            (x - name_hx[0].0).abs() <= 2.0,
            "row {i} shares the header column x ({x} vs {})",
            name_hx[0].0
        );
        prev_bottom = y + h;
    }
    assert_eq!(cell_calls.get(), 8, "one counted-cell call per mounted row");
    assert_eq!(header_calls.get(), 1, "header factory runs once");
    // Roles + totals through core metadata.
    let sem = tree.emit_semantics();
    assert_eq!(
        sem.iter().filter(|n| n.role == Role::ColumnHeader).count(),
        2,
        "one ColumnHeader per column"
    );
    assert!(
        sem.iter().any(|n| n.role == Role::Row),
        "body rows carry Row roles"
    );
    assert!(
        sem.iter().any(|n| n
            .collection
            .is_some_and(|cs| cs.row_count == Some(8) && cs.column_count == Some(2))),
        "table totals visible"
    );
    for n in sem.iter().filter(|x| x.role == Role::Cell) {
        let cs = n.collection.as_ref().expect("cell coordinates");
        assert!(
            cs.column_index == Some(0) || cs.column_index == Some(1),
            "zero-based column index, got {cs:?}"
        );
    }
}

#[test]
fn virtualized_table_bounds_and_stacks_vertically() {
    let cell_calls = Rc::new(Cell::new(0));
    let header_calls = Rc::new(Cell::new(0));
    let items: Vec<TCell> = (0..5000u64)
        .map(|i| TCell {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let mut tree = UiTree::new();
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let mut view = View::new(scope, tree.command_sink());
    view.set_content(
        &mut tree,
        vec![table_element(
            items,
            cell_calls.clone(),
            header_calls.clone(),
            true,
        )],
    )
    .unwrap();
    drive(&mut view, &mut tree, 700.0, 600.0);
    drive(&mut view, &mut tree, 700.0, 600.0);
    assert!(
        cell_calls.get() < 600,
        "cell factories bounded, got {}",
        cell_calls.get()
    );
    // Mounted body rows stack vertically with increasing y.
    let mut ys: Vec<f32> = (0..20)
        .flat_map(|i| cell_rects(&tree, &format!("name-{i}")))
        .map(|(_, y, _, _)| y)
        .collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(ys.len() > 4, "head rows mounted");
    assert!(
        ys.windows(2).all(|w| w[1] > w[0]),
        "mounted rows stack vertically, got {ys:?}"
    );
}

// ---------------------------------------------------------------------------
// 11. Semantics: roles, totals, logical indices.
// ---------------------------------------------------------------------------

#[test]
fn semantics_carry_roles_totals_and_indices() {
    let source = Rc::new(TSource::plain(500));
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, true, &source);
    drive(&mut view, &mut tree, 600.0, 400.0);
    drive(&mut view, &mut tree, 600.0, 400.0);
    let sem = tree.emit_semantics();
    let rows: Vec<_> = sem.iter().filter(|n| n.role == Role::ListItem).collect();
    assert!(!rows.is_empty(), "mounted rows carry ListItem roles");
    for n in &rows {
        let cs = n.collection.as_ref().expect("row coordinates");
        assert_eq!(cs.set_size, Some(500));
        // ListItem wrappers carry the role, not the text: derive the
        // logical index from descendant text (triaged contract).
        let idx: usize = descendant_texts(&tree, n.node)
            .iter()
            .find_map(|l| l.strip_prefix("row-")?.parse().ok())
            .expect("row descendant text parses");
        assert_eq!(cs.position_in_set, Some(idx + 1), "one-based positions");
    }
}

// ---------------------------------------------------------------------------
// 12. Zero-height viewport mounts a minimal window, never all rows.
// ---------------------------------------------------------------------------

#[test]
fn zero_viewport_mounts_minimal_window() {
    let source = Rc::new(TSource::plain(10_000));
    let mut tree = UiTree::new();
    let (_rt, mut view) = mount_new(&mut tree, true, &source);
    view.compute_layout(&mut tree, Size::new(0.0, 0.0), &NaiveMeasurer)
        .expect("zero viewport drives");
    view.compute_layout(&mut tree, Size::new(0.0, 0.0), &NaiveMeasurer)
        .expect("zero viewport drives");
    assert!(
        row_texts(&tree).len() <= 9,
        "minimal anchor window at zero viewport"
    );
    assert!(
        source.row_calls.get() <= 9,
        "factories bounded at zero viewport"
    );
}

// ---------------------------------------------------------------------------
// 13. Focus survives keyed reorder with edits intact (unvirtualized).
// ---------------------------------------------------------------------------

type SigPair = (ReadSignal<String>, WriteSignal<String>);

struct EditSource {
    items: Vec<u64>,
    sigs: Rc<Vec<SigPair>>,
}

impl CollectionSource for EditSource {
    fn len(&self) -> usize {
        self.items.len()
    }
    fn key(&self, index: usize) -> Key {
        self.items[index].into()
    }
    fn layout(&self, w: f32) -> CollectionLayout {
        CollectionLayout {
            content_width: w.max(0.0),
            column_widths: Vec::new(),
        }
    }
    fn row(&self, index: usize, _layout: &CollectionLayout) -> Element {
        let k = self.items[index];
        let (r, w) = &self.sigs[k as usize];
        ui::Text::builder().bind((r.clone(), w.clone())).build()
    }
    fn header(&self, _layout: &CollectionLayout) -> Option<Element> {
        None
    }
    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }
    fn validate(&self) -> Result<(), CollectionError> {
        Ok(())
    }
}

#[test]
fn reorder_preserves_focus_and_edits() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let sigs: Rc<Vec<SigPair>> = Rc::new(
        (0..8u64)
            .map(|i| scope.signal(format!("item-{i}")))
            .collect(),
    );
    let source: Rc<dyn CollectionSource> = Rc::new(EditSource {
        items: (0..8).collect(),
        sigs: Rc::clone(&sigs),
    });
    let mut tree = UiTree::new();
    let scope2 = rt.create_scope();
    let mut view = View::new(scope2, tree.command_sink());
    view.set_content(
        &mut tree,
        vec![Element::collection(CollectionDescriptor::new(
            Rc::clone(&source),
            Vec::new(),
            false,
            32.0,
            4,
        ))],
    )
    .unwrap();
    drive(&mut view, &mut tree, 600.0, 800.0);
    let target = tree
        .traverse()
        .into_iter()
        .find(|n| {
            matches!(tree.get(*n).map(|r| &r.kind), Some(aqua_ui::tree::NodeKind::Text(t)) if t.text() == "item-5")
        })
        .expect("editable row mounted");
    assert!(tree.set_focusable(target, true));
    assert!(tree.set_focused(Some(target)));
    tree.fire_action(ActionEvent {
        source: None,
        node: target,
        kind: ActionKind::TextChanged,
        text: Some("item-5!".to_string()),
        focused: None,
    });
    assert_eq!(sigs[5].0.get(), "item-5!");
    let source2: Rc<dyn CollectionSource> = Rc::new(EditSource {
        items: (0..8u64).rev().collect(),
        sigs: Rc::clone(&sigs),
    });
    view.set_content(
        &mut tree,
        vec![Element::collection(CollectionDescriptor::new(
            source2,
            Vec::new(),
            false,
            32.0,
            4,
        ))],
    )
    .unwrap();
    drive(&mut view, &mut tree, 600.0, 800.0);
    assert_eq!(tree.focused(), Some(target), "focus follows the key");
    match tree.get(target).map(|r| &r.kind) {
        Some(aqua_ui::tree::NodeKind::Text(t)) => assert_eq!(t.text(), "item-5!"),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn table_horizontal_scroll_moves_header_and_body_together() {
    let items: Vec<TCell> = (0..200u64)
        .map(|i| TCell {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let mut tree = UiTree::new();
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let mut view = View::new(scope, tree.command_sink());
    // Agreeing horizontal chrome (header gap 6 == row gap 6): the shared
    // plan requires it, and validation rejects disagreement loudly.
    {
        use aqua_ui::collection::{ColumnBuilder, Table};
        use aqua_ui::style::{Length, Style};
        view.set_content(
            &mut tree,
            vec![
                Table::new()
                    .items(items)
                    .key(|it: &TCell| it.key)
                    .columns([
                        ColumnBuilder::new("name")
                            .header(|| ui::Text::builder().arg("Name").build())
                            .cell(|it: &TCell| ui::Text::builder().arg(it.name.clone()).build())
                            .width(Length::Px(400.0))
                            .build(),
                        ColumnBuilder::new("value")
                            .header(|| ui::Text::builder().arg("Value").build())
                            .cell(|it: &TCell| {
                                ui::Text::builder().arg(format!("{}", it.value)).build()
                            })
                            .width(Length::Px(400.0))
                            .build(),
                    ])
                    .header_styles([Style::new().gap(6.0)])
                    .row_styles([Style::new().gap(6.0)])
                    .virtualize()
                    .build(),
            ],
        )
        .unwrap();
    }
    drive(&mut view, &mut tree, 700.0, 600.0);
    drive(&mut view, &mut tree, 700.0, 600.0);
    // Header/body share one width plan in screen space; header visible
    // (nonzero height, no overlap, above the first body row).
    let name_hx = visual_x_of(&tree, "Name");
    let name_bx = visual_x_of(&tree, "name-0");
    assert!(
        (name_hx - name_bx).abs() <= 2.0,
        "header/body share the width plan: {name_hx} vs {name_bx}"
    );
    assert_header_visible_at(&tree, "Name", "name-0");
    // Horizontal scroll on the body viewport moves header and body together.
    let body = tree
        .traverse()
        .into_iter()
        .find(|n| {
            matches!(tree.get(*n).map(|r| &r.kind), Some(NodeKind::Scroll(_)))
                && tree
                    .get(*n)
                    .map(|r| !r.children.is_empty())
                    .unwrap_or(false)
        })
        .expect("body scroll viewport");
    assert!(tree.scroll_to(body, Point::new(80.0, 0.0)));
    drive(&mut view, &mut tree, 700.0, 600.0);
    drive(&mut view, &mut tree, 700.0, 600.0);
    let name_hx2 = visual_x_of(&tree, "Name");
    let name_bx2 = visual_x_of(&tree, "name-0");
    for (before, after, what) in [
        (name_hx, name_hx2, "Name header"),
        (name_bx, name_bx2, "name body"),
    ] {
        assert!(
            (before - after - 80.0).abs() <= 3.0,
            "{what} scrolled with the body: {before} -> {after}"
        );
    }
    assert!(
        (name_hx2 - name_bx2).abs() <= 2.0,
        "shared plan holds while scrolled"
    );
    assert_header_visible_at(&tree, "Name", "name-0");
}

#[test]
fn pixel_overscan_and_viewport_reports_follow_measured_geometry() {
    use aqua_ui::collection_runtime::CollectionState;
    use std::cell::RefCell;
    let source = Rc::new(TSource::plain(100));
    let reports = Rc::new(RefCell::new(Vec::new()));
    let capture = Rc::clone(&reports);
    let mut descriptor = desc_of(&source, true);
    descriptor.overscan_pixels = Some(50.0);
    descriptor.on_viewport = Some(Rc::new(move |report| capture.borrow_mut().push(report)));
    let mut state = CollectionState::new(descriptor).unwrap();
    state.set_viewport(400.0, 64.0);
    state.set_scroll(0.0, 320.0);
    let window = state.desired_window();
    assert_eq!((window.start, window.end), (8, 14));
    state.set_mounted(window);
    let report = *reports.borrow().last().unwrap();
    assert_eq!((report.visible.start, report.visible.end), (10, 13));
    assert_eq!(report.distance_from_start, 320.0);
    assert_eq!(report.distance_from_end, 2816.0);
    state.set_scroll(0.0, 321.0);
    assert_eq!(reports.borrow().last().unwrap().scroll_y, 321.0);
    assert!(state.note_measured(8, 64.0, 0.0, 0.0));
    assert_eq!(state.scroll().1, 353.0);
}

#[test]
fn initial_row_heights_and_reveal_use_cumulative_geometry() {
    use aqua_ui::collection_runtime::CollectionState;
    let source = Rc::new(TSource::plain(100));
    let mut descriptor = desc_of(&source, true);
    descriptor.initial_height = Some(Rc::new(|index| if index == 0 { 100.0 } else { 20.0 }));
    let mut state = CollectionState::new(descriptor).unwrap();
    state.set_viewport(400.0, 60.0);
    assert_eq!(state.offset_of(10), 280.0);
    assert!(state.reveal_index(10));
    assert_eq!(state.scroll().1, 240.0);
    assert!(state.note_measured(0, 120.0, 0.0, 0.0));
    assert_eq!(state.scroll().1, 260.0);
    assert!(!state.reveal_index(100));
}

#[test]
fn aligned_reveal_uses_fixed_and_uniform_geometry_and_waits_for_measurement() {
    use aqua_ui::collection_runtime::{CollectionRevealAlignment, CollectionState};
    let source = Rc::new(TSource::plain(100));
    let mut descriptor = desc_of(&source, true);
    descriptor.fixed_row_height = true;
    let mut fixed = CollectionState::new(descriptor).unwrap();
    assert!(!fixed.reveal_index_aligned(5, CollectionRevealAlignment::Start));
    fixed.set_viewport(200.0, 96.0);
    assert!(fixed.reveal_index_aligned(10, CollectionRevealAlignment::Start));
    assert_eq!(fixed.scroll().1, 320.0);
    assert!(fixed.reveal_index_aligned(11, CollectionRevealAlignment::Nearest));
    assert_eq!(fixed.scroll().1, 320.0);
    assert!(fixed.reveal_index_aligned(11, CollectionRevealAlignment::Start));
    assert_eq!(fixed.scroll().1, 352.0);
    assert!(fixed.reveal_index_aligned(15, CollectionRevealAlignment::End));
    assert_eq!(fixed.scroll().1, 416.0);
    assert!(fixed.reveal_index_aligned(99, CollectionRevealAlignment::Start));
    assert_eq!(fixed.scroll().1, 3104.0);
    assert!(!fixed.reveal_index_aligned(100, CollectionRevealAlignment::Start));

    let mut descriptor = desc_of(&source, true);
    descriptor.measured_uniform_rows = true;
    let mut uniform = CollectionState::new(descriptor).unwrap();
    uniform.set_viewport(200.0, 96.0);
    assert_eq!(
        uniform.reveal_target_at(10, CollectionRevealAlignment::Start, 0.0),
        None
    );
    assert!(!uniform.reveal_index_aligned(10, CollectionRevealAlignment::Start));
    assert!(uniform.note_measured(0, 24.0, 0.0, 0.0));
    assert_eq!(
        uniform.reveal_target_at(10, CollectionRevealAlignment::Start, 0.0),
        Some(240.0)
    );
    assert_eq!(uniform.scroll().1, 0.0);
    assert!(uniform.reveal_index_aligned(10, CollectionRevealAlignment::Start));
    assert_eq!(uniform.scroll().1, 240.0);
    assert!(uniform.reveal_index_aligned(99, CollectionRevealAlignment::Start));
    assert_eq!(uniform.scroll().1, 2304.0);
}

#[test]
fn aligned_view_reveal_waits_for_a_committed_viewport() {
    use aqua_ui::collection_runtime::CollectionRevealAlignment;
    let source = Rc::new(TSource::plain(100));
    let mut descriptor = desc_of(&source, true);
    descriptor.fixed_row_height = true;
    let mut tree = UiTree::new();
    let runtime = Runtime::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![Element::collection(descriptor)])
        .unwrap();
    let root = view.root().unwrap();
    assert!(!view.reveal_collection_index_aligned(
        &mut tree,
        root,
        10,
        CollectionRevealAlignment::Start
    ));
    assert!(
        view.collection_index_target_aligned(&tree, root, 10, CollectionRevealAlignment::Start)
            .is_none()
    );
    drive(&mut view, &mut tree, 200.0, 160.0);
    let original = tree.scroll_offset(root).unwrap();
    assert_eq!(tree.content_size(root).unwrap().h, 3200.0);
    assert_eq!(
        view.collection_index_target_aligned(&tree, root, 10, CollectionRevealAlignment::Start)
            .unwrap()
            .y,
        320.0
    );
    assert_eq!(tree.scroll_offset(root).unwrap(), original);
    assert_eq!(
        view.collection_index_target_aligned(&tree, root, 99, CollectionRevealAlignment::Start)
            .unwrap()
            .y,
        3040.0
    );
    assert!(tree.scroll_to(root, Point::new(0.0, 3040.0)));
    assert_eq!(tree.scroll_offset(root).unwrap().y, 3040.0);
    assert!(tree.scroll_to(root, Point::new(0.0, 320.0)));
    assert_eq!(
        view.collection_index_target_aligned(&tree, root, 11, CollectionRevealAlignment::Nearest)
            .unwrap()
            .y,
        320.0
    );
    assert_eq!(tree.scroll_offset(root).unwrap().y, 320.0);
    assert!(tree.scroll_to(root, Point::new(0.0, 0.0)));
    assert!(view.reveal_collection_index_aligned(
        &mut tree,
        root,
        10,
        CollectionRevealAlignment::Start
    ));
    assert_eq!(tree.scroll_offset(root).unwrap().y, 320.0);
}

#[test]
fn stable_indexed_collection_does_not_allocate_or_scan_all_rows() {
    use aqua_ui::collection_runtime::CollectionState;
    struct Huge {
        keys: Cell<usize>,
    }
    impl CollectionSource for Huge {
        fn len(&self) -> usize {
            1_000_000
        }
        fn stable_indexed(&self) -> bool {
            true
        }
        fn key(&self, index: usize) -> Key {
            self.keys.set(self.keys.get() + 1);
            Key::Usize(index)
        }
        fn layout(&self, available_width: f32) -> CollectionLayout {
            CollectionLayout {
                content_width: available_width,
                column_widths: Vec::new(),
            }
        }
        fn row(&self, _: usize, _: &CollectionLayout) -> Element {
            ui::Box::builder().build()
        }
        fn header(&self, _: &CollectionLayout) -> Option<Element> {
            None
        }
        fn role(&self) -> CollectionRole {
            CollectionRole::List
        }
        fn validate(&self) -> Result<(), CollectionError> {
            Ok(())
        }
    }
    let source = Rc::new(Huge { keys: Cell::new(0) });
    let descriptor = CollectionDescriptor::new(source.clone(), Vec::new(), true, 32.0, 4);
    let mut state = CollectionState::new(descriptor.clone()).unwrap();
    assert_eq!(source.keys.get(), 0);
    state.set_viewport(400.0, 128.0);
    assert!(state.reveal_index(500_000));
    let window = state.desired_window();
    assert!(window.contains(500_000));
    state.set_mounted(window);
    assert!(source.keys.get() < 100);
    assert!(state.note_measured(window.start, 40.0, 0.0, 0.0));
    state.refresh(descriptor).unwrap();
    assert!(state.desired_window().contains(500_000));
    assert!(source.keys.get() < 200);
}

#[test]
fn fixed_height_policy_ignores_row_measurements() {
    use aqua_ui::collection_runtime::CollectionState;
    let source = Rc::new(TSource::plain(50));
    let mut descriptor = desc_of(&source, true);
    descriptor.fixed_row_height = true;
    let mut state = CollectionState::new(descriptor).unwrap();
    state.set_viewport(400.0, 96.0);
    state.set_scroll(0.0, 320.0);
    assert!(!state.note_measured(0, 100.0, 0.0, 0.0));
    assert_eq!(state.offset_of(10), 320.0);
    assert_eq!(state.scroll().1, 320.0);
}

#[test]
fn uniform_rows_use_first_row_measurement_and_keep_the_scroll_anchor() {
    use aqua_ui::collection_runtime::CollectionState;
    let source = Rc::new(TSource::plain(100));
    let mut descriptor = desc_of(&source, true);
    descriptor.measured_uniform_rows = true;
    let mut state = CollectionState::new(descriptor).unwrap();
    state.set_viewport(400.0, 96.0);
    state.set_scroll(0.0, 325.0);
    let runs = state.mounted_runs(state.desired_window());
    assert_eq!(runs[0].start, 0);
    assert_eq!(runs[0].end, 1);
    assert!(!state.note_measured(10, 80.0, 0.0, 0.0));
    assert!(state.note_measured(0, 40.0, 0.0, 0.0));
    assert_eq!(state.offset_of(10), 400.0);
    assert_eq!(state.scroll().1, 405.0);
    assert!(!state.note_measured(1, 90.0, 0.0, 0.0));
    state.set_viewport(500.0, 96.0);
    assert!(state.note_measured(0, 20.0, 0.0, 0.0));
    assert_eq!(state.offset_of(10), 200.0);
    assert_eq!(state.scroll().1, 205.0);
}

#[test]
fn viewport_callback_waits_for_a_real_viewport() {
    use aqua_ui::collection_runtime::CollectionState;
    use std::cell::RefCell;
    let source = Rc::new(TSource::plain(100));
    let reports = Rc::new(RefCell::new(Vec::new()));
    let capture = Rc::clone(&reports);
    let mut descriptor = desc_of(&source, true);
    descriptor.on_viewport = Some(Rc::new(move |report| capture.borrow_mut().push(report)));
    let mut state = CollectionState::new(descriptor).unwrap();
    state.set_viewport(0.0, 0.0);
    state.set_scroll(0.0, 0.0);
    state.set_mounted(state.desired_window());
    assert!(reports.borrow().is_empty());
    state.set_viewport(400.0, 96.0);
    assert!(!reports.borrow().is_empty());
}

struct RetentionHarness {
    tree: UiTree,
    view: View,
    _runtime: Runtime,
    built: Rc<std::cell::RefCell<Vec<u64>>>,
}

impl RetentionHarness {
    fn new() -> Self {
        let tree = UiTree::new();
        let runtime = Runtime::new();
        let view = View::new(runtime.create_scope(), tree.command_sink());
        Self {
            tree,
            view,
            _runtime: runtime,
            built: Rc::default(),
        }
    }

    fn list(&self, items: Vec<(u64, Option<u64>)>) -> Element {
        self.list_with(items, None, Rc::default())
    }

    fn list_with(
        &self,
        items: Vec<(u64, Option<u64>)>,
        epoch: Option<u64>,
        key_calls: Rc<Cell<usize>>,
    ) -> Element {
        use aqua_ui::collection::{BatchedCollectionRow, List};
        use aqua_ui::style::{Length, Style};
        let built = Rc::clone(&self.built);
        let mut list = List::new();
        if let Some(epoch) = epoch {
            list = list.shape_epoch(epoch);
        }
        Element::collection(
            list.items(items)
                .key(move |(key, _)| {
                    key_calls.set(key_calls.get() + 1);
                    *key
                })
                .revision(|(_, revision)| *revision)
                .try_batched_row_with_layout(move |(key, _), _| {
                    built.borrow_mut().push(*key);
                    Ok(BatchedCollectionRow {
                        element: ui::Box::builder()
                            .height(32.0)
                            .child(ui::Text::builder().arg(format!("row-{key}")).build())
                            .build(),
                        batches: Vec::new(),
                    })
                })
                .style(
                    Style::new()
                        .width(Length::Pct(100.0))
                        .height(Length::Px(200.0)),
                )
                .virtualize()
                .descriptor(),
        )
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            drive(&mut self.view, &mut self.tree, 600.0, 400.0);
        }
    }

    fn node_of(&self, key: u64) -> Option<NodeId> {
        text_node(&self.tree, &format!("row-{key}"))
    }
}

#[test]
fn refresh_keeps_mounted_rows_with_equal_revisions() {
    let mut harness = RetentionHarness::new();
    let items: Vec<_> = (0..100u64).map(|key| (key, Some(key))).collect();
    let content = harness.list(items.clone());
    harness
        .view
        .set_content(&mut harness.tree, vec![content])
        .unwrap();
    harness.settle();
    let mounted: Vec<u64> = harness.built.borrow().clone();
    assert!(mounted.len() >= 4 && mounted.len() < 40);
    let before: Vec<_> = mounted
        .iter()
        .map(|key| harness.node_of(*key).unwrap())
        .collect();
    harness.built.borrow_mut().clear();

    let mut next: Vec<_> = (1000..1005u64).map(|key| (key, Some(key))).collect();
    next.extend(items);
    let content = harness.list(next);
    harness
        .view
        .set_content(&mut harness.tree, vec![content])
        .unwrap();
    harness.settle();
    assert!(
        harness
            .built
            .borrow()
            .iter()
            .all(|key| *key >= 1000 || !mounted.contains(key)),
        "rows mounted before the refresh were rebuilt: {:?}",
        harness.built.borrow()
    );
    let after: Vec<_> = mounted
        .iter()
        .map(|key| harness.node_of(*key).unwrap())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn refresh_rebuilds_rows_whose_revision_changed_or_is_unknown() {
    let mut harness = RetentionHarness::new();
    let items: Vec<_> = (0..100u64).map(|key| (key, Some(key))).collect();
    let content = harness.list(items.clone());
    harness
        .view
        .set_content(&mut harness.tree, vec![content])
        .unwrap();
    harness.settle();
    harness.built.borrow_mut().clear();

    let mut next = items;
    next[1].1 = Some(9_000);
    next[2].1 = None;
    let kept = harness.node_of(0).unwrap();
    let content = harness.list(next);
    harness
        .view
        .set_content(&mut harness.tree, vec![content])
        .unwrap();
    harness.settle();
    let mut rebuilt = harness.built.borrow().clone();
    rebuilt.sort_unstable();
    assert_eq!(rebuilt, vec![1, 2]);
    assert_eq!(harness.node_of(0), Some(kept));
}

#[test]
fn refresh_without_revisions_rebuilds_every_mounted_row() {
    let mut harness = RetentionHarness::new();
    let items: Vec<_> = (0..100u64).map(|key| (key, None)).collect();
    let content = harness.list(items.clone());
    harness
        .view
        .set_content(&mut harness.tree, vec![content])
        .unwrap();
    harness.settle();
    let mounted = harness.built.borrow().len();
    harness.built.borrow_mut().clear();
    let content = harness.list(items);
    harness
        .view
        .set_content(&mut harness.tree, vec![content])
        .unwrap();
    harness.settle();
    assert_eq!(harness.built.borrow().len(), mounted);
}

#[test]
fn matching_shape_epoch_skips_the_snapshot_refresh_and_still_checks_revisions() {
    let mut harness = RetentionHarness::new();
    let items: Vec<_> = (0..500u64).map(|key| (key, Some(key))).collect();
    let calls = Rc::new(Cell::new(0));
    let content = harness.list_with(items.clone(), Some(1), Rc::clone(&calls));
    harness
        .view
        .set_content(&mut harness.tree, vec![content])
        .unwrap();
    harness.settle();
    let kept = harness.node_of(0).unwrap();
    harness.built.borrow_mut().clear();
    calls.set(0);

    let mut next = items.clone();
    next[1].1 = Some(9_000);
    let content = harness.list_with(next, Some(1), Rc::clone(&calls));
    harness
        .view
        .set_content(&mut harness.tree, vec![content])
        .unwrap();
    harness.settle();
    assert!(
        calls.get() < 100,
        "refresh scanned the snapshot: {}",
        calls.get()
    );
    assert_eq!(*harness.built.borrow(), vec![1]);
    assert_eq!(harness.node_of(0), Some(kept));

    calls.set(0);
    let content = harness.list_with(items, Some(2), Rc::clone(&calls));
    harness
        .view
        .set_content(&mut harness.tree, vec![content])
        .unwrap();
    harness.settle();
    assert!(calls.get() >= 500);
}
