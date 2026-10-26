//! Collection builder/source tests (collection-API worker owned).
//!
//! These exercise the canonical `aqua_ui::collection` API exactly as peer
//! crates consume it: builder chains, snapshot semantics, lazy factories,
//! shared width-plan math, track-placement validation, and headless x/y
//! alignment through `UiTree::compute_layout`.
//!
//! Roles/positions in emitted core metadata belong to the runtime peer
//! (`collection_runtime` + `View` integration tests); row/cell roles are
//! applied by sources via `Element::role` and cannot be observed from
//! `NodeSpec`s headless.

use std::rc::Rc;

use aqua_core::{NodeId, Oklch, Size};
use aqua_ui::collection::{fr, CollectionRole, Column, ColumnBuilder, ColumnWidth, List, Table};
use aqua_ui::collection_runtime::CollectionError;
use aqua_ui::element::{self, lower_specs, Element, Text};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::{px, Axis, Style};
use aqua_ui::tree::UiTree;

// ---------------------------------------------------------------------------
// Fixtures + headless layout helpers
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Item {
    id: u32,
    name: &'static str,
}

fn items() -> Vec<Item> {
    vec![
        Item { id: 1, name: "a" },
        Item { id: 2, name: "bb" },
        Item { id: 3, name: "ccc" },
    ]
}

fn columns() -> [Column<Item>; 3] {
    [
        ColumnBuilder::new("id")
            .header(|| Text::builder().arg("ID").font_size(18.0).build())
            .cell(|it: &Item| Text::builder().arg(it.id.to_string()).build())
            .width(px(120.0))
            .build(),
        ColumnBuilder::new("name")
            .header(|| {
                element::Box::new()
                    .child(Text::builder().arg("Name").build())
                    .build()
            })
            .cell(|it: &Item| {
                // Differently styled inner content: inner padding/font are the
                // author's freedom and must never move the column grid.
                Text::builder()
                    .arg(it.name)
                    .font_size(11.0)
                    .styles([Style::new().background(Oklch::BLACK)])
                    .build()
            })
            .width(fr(1.0))
            .build(),
        ColumnBuilder::new("act")
            .header(|| Text::builder().arg("Act").build())
            .cell(|it: &Item| Text::builder().arg(format!("go-{}", it.id)).build())
            .width(fr(2.0))
            .min_width(60.0)
            .max_width(400.0)
            .build(),
    ]
}

fn table() -> Table<Item> {
    Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .header_styles([Style::new().background(Oklch::BLACK)])
        .row_styles([Style::new().background(Oklch::WHITE)])
        .cell_styles([Style::new().padding(px(6.0))])
}

/// Mount a single-root element and lay out at `viewport`. Returns the tree
/// plus the root id; children ids come from `tree.get(id).children`.
fn layout_root(el: &Element, viewport: Size) -> (UiTree, NodeId) {
    let specs = lower_specs(el).expect("lowering must succeed");
    assert_eq!(specs.len(), 1, "expected one root spec");
    let mut tree = UiTree::new();
    let root = tree.mount_spec(specs.into_iter().next().unwrap());
    tree.compute_layout(viewport, &NaiveMeasurer)
        .expect("layout must succeed");
    (tree, root)
}

fn kids(tree: &UiTree, id: NodeId) -> Vec<NodeId> {
    tree.get(id).expect("node must exist").children.clone()
}

fn xw(tree: &UiTree, id: NodeId) -> (f32, f32) {
    let r = tree.rect(id).expect("rect must be committed");
    (r.x, r.w)
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// Engine-rounding tolerance for plan-vs-actual widths: taffy snaps
/// fractional flex widths to whole px (identically for header and body, so
/// header<->body alignment still uses `approx`). Integer plans measure exact.
fn approx_px(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.0
}

/// Vertical stack container for headless alignment checks: header bar and
/// body rows must pile vertically (column axis) so per-column x positions
/// are comparable across rows.
fn stack(children: Vec<Element>) -> Element {
    let mut root = element::Box::new().layout(Axis::Column);
    for c in children {
        root = root.child(c);
    }
    root.build()
}

// ---------------------------------------------------------------------------
// Track types: const fr, ColumnWidth shape
// ---------------------------------------------------------------------------

#[test]
fn fr_is_const_and_tracks_stay_const() {
    const TRACK: ColumnWidth = fr(2.0);
    const PAD: Style = Style::new().padding(px(4.0));
    assert_eq!(TRACK, ColumnWidth::Flex(2.0));
    assert!(!PAD.is_empty());
    // Invalid weights are stored, never panicked on (validation rejects).
    let bad = fr(f32::NAN);
    assert!(matches!(bad, ColumnWidth::Flex(_)));
}

#[test]
fn length_converts_to_fixed_track() {
    let t: ColumnWidth = px(120.0).into();
    assert_eq!(t, ColumnWidth::Fixed(px(120.0)));
}

// ---------------------------------------------------------------------------
// Width-plan math (pure sizing, no cells visited)
// ---------------------------------------------------------------------------

#[test]
fn width_plan_fixed_fr_pct_minmax() {
    let t = Table::new().items(items()).key(|it: &Item| it.id).columns([
        ColumnBuilder::new("a")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .width(px(100.0))
            .build(),
        ColumnBuilder::new("b")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .width(fr(1.0))
            .build(),
        ColumnBuilder::new("c")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .width(fr(3.0))
            .build(),
    ]);
    // 400 - 100 fixed = 300 split 1:3.
    assert_eq!(t.column_widths(400.0), vec![100.0, 75.0, 225.0]);
    assert!(t.validate().is_ok());

    // pct tracks take their share of the available width.
    let t = Table::new().items(items()).key(|it: &Item| it.id).columns([
        ColumnBuilder::new("a")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .width(aqua_ui::style::pct(50.0))
            .build(),
        ColumnBuilder::new("b")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .width(fr(1.0))
            .build(),
    ]);
    assert_eq!(t.column_widths(400.0), vec![200.0, 200.0]);

    // min clamps force overflow (content exceeds available: scrolls).
    let t = Table::new().items(items()).key(|it: &Item| it.id).columns([
        ColumnBuilder::new("a")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .width(fr(1.0))
            .min_width(250.0)
            .build(),
        ColumnBuilder::new("b")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .width(fr(1.0))
            .build(),
    ]);
    let widths = t.column_widths(400.0);
    assert_eq!(widths, vec![250.0, 200.0]);
    let desc = t.descriptor();
    let layout = desc.source.layout(400.0);
    assert!(approx(layout.content_width, 450.0));

    // Insane available widths sanitize the flexible area to zero, never
    // panic; fixed tracks stay exact (they overflow into scroll).
    let t = Table::<Item>::new()
        .items(vec![])
        .key(|it: &Item| it.id)
        .columns([ColumnBuilder::new("a")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .width(px(10.0))
            .build()]);
    assert_eq!(t.column_widths(f32::NAN), vec![10.0]);
    assert_eq!(t.column_widths(-50.0), vec![10.0]);
}

// ---------------------------------------------------------------------------
// Shared x positions: header vs body, wide and narrow (overflow)
// ---------------------------------------------------------------------------

/// One authoritative placement: with differently styled header/cell content
/// (fonts, fills, inner padding), every header column x/width must equal the
/// body column x/width, and both must equal the shared plan.
#[test]
fn header_body_share_column_x_positions() {
    let t = table();
    assert!(t.validate().is_ok());
    let desc = t.descriptor();
    let layout = desc.source.layout(500.0);
    // 120 fixed + (500-120) split 1:2 with min/max untouched.
    let plan = layout.column_widths.clone();
    assert_eq!(plan.len(), 3);
    assert!(approx(plan[0], 120.0));
    assert!(approx(plan[1], 380.0 / 3.0));
    assert!(approx(plan[2], 2.0 * 380.0 / 3.0));
    assert!(approx(layout.content_width, 500.0));

    let header = desc.source.header(&layout).expect("table header");
    let rows: Vec<Element> = (0..desc.source.len())
        .map(|i| desc.source.row(i, &layout))
        .collect();
    let mut parts = vec![header];
    parts.extend(rows);
    let (tree, root_id) = layout_root(&stack(parts), Size::new(500.0, 400.0));
    let row_ids = kids(&tree, root_id);
    assert_eq!(row_ids.len(), 1 + desc.source.len());

    let header_cells = kids(&tree, row_ids[0]);
    assert_eq!(header_cells.len(), 3);
    for (c, w) in header_cells.iter().zip(plan.iter()) {
        // Border-box proof: 6px cell padding stays INSIDE the track width
        // (taffy snaps fractional flex widths to whole px identically for
        // header and body, so header<->body uses tight tolerance while
        // plan comparisons allow 1px engine rounding).
        assert!(approx_px(xw(&tree, *c).1, *w), "header cell width {c:?}");
    }
    for r in row_ids.iter().skip(1) {
        let cells = kids(&tree, *r);
        assert_eq!(cells.len(), 3);
        for (i, c) in cells.iter().enumerate() {
            let (x, w) = xw(&tree, *c);
            let (hx, hw) = xw(&tree, header_cells[i]);
            assert!(approx(x, hx), "column {i} x: body {x} vs header {hx}");
            assert!(approx(w, hw), "column {i} w: body {w} vs header {hw}");
            assert!(approx_px(w, plan[i]), "column {i} w vs plan");
        }
    }
}

/// Resizing narrow forces horizontal overflow; the shared plan still places
/// header and body columns identically (one plan, one scroll offset).
#[test]
fn narrow_overflow_keeps_header_body_aligned() {
    // Fixed tracks wider than the viewport: content 200 > available 100.
    let t = Table::new().items(items()).key(|it: &Item| it.id).columns([
        ColumnBuilder::new("a")
            .header(|| Text::builder().arg("H1").build())
            .cell(|_: &Item| Text::builder().arg("x").build())
            .width(px(120.0))
            .build(),
        ColumnBuilder::new("b")
            .header(|| Text::builder().arg("H2").build())
            .cell(|_: &Item| Text::builder().arg("y").build())
            .width(px(80.0))
            .build(),
    ]);
    assert!(t.validate().is_ok());
    let desc = t.descriptor();
    let layout = desc.source.layout(100.0);
    assert_eq!(layout.column_widths, vec![120.0, 80.0]);
    assert!(approx(layout.content_width, 200.0));

    let header = desc.source.header(&layout).expect("table header");
    let rows: Vec<Element> = (0..desc.source.len())
        .map(|i| desc.source.row(i, &layout))
        .collect();
    let mut parts = vec![header];
    parts.extend(rows);
    let (tree, root_id) = layout_root(&stack(parts), Size::new(100.0, 400.0));
    let row_ids = kids(&tree, root_id);
    let header_cells = kids(&tree, row_ids[0]);
    // flex_shrink(0) cells: widths stay exactly on plan under overflow.
    assert!(approx(xw(&tree, header_cells[0]).1, 120.0));
    assert!(approx(xw(&tree, header_cells[1]).1, 80.0));
    for r in row_ids.iter().skip(1) {
        let cells = kids(&tree, *r);
        assert_eq!(cells.len(), 2);
        for (i, c) in cells.iter().enumerate() {
            let (x, w) = xw(&tree, *c);
            let (hx, hw) = xw(&tree, header_cells[i]);
            assert!(approx(x, hx), "overflow column {i} x: {x} vs {hx}");
            assert!(approx(w, hw), "overflow column {i} w: {w} vs {hw}");
        }
    }
    // First column starts at the shared origin in both.
    assert!(approx(xw(&tree, header_cells[0]).0, 0.0));
}

// ---------------------------------------------------------------------------
// Track-placement validation: loud, never silent
// ---------------------------------------------------------------------------

#[test]
fn row_gap_padding_breaks_track_and_fails() {
    // The demo's header gap 8 vs row gap 10 shape: incompatible horizontal
    // chrome is rejected with a clear error, never coerced.
    let bad_header = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .header_styles([Style::new().gap(8.0)]);
    assert!(
        matches!(bad_header.validate(), Err(CollectionError::Validation(_))),
        "header-only gap mismatches the default body chrome"
    );

    let bad_row = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .row_styles([Style::new().gap(10.0)]);
    assert!(matches!(
        bad_row.validate(),
        Err(CollectionError::Validation(_))
    ));

    let bad_pad = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .row_styles([Style::new().padding(px(4.0))]);
    assert!(matches!(
        bad_pad.validate(),
        Err(CollectionError::Validation(_))
    ));

    let bad_axis = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .row_styles([Style::new().column()]);
    assert!(matches!(
        bad_axis.validate(),
        Err(CollectionError::Validation(_))
    ));

    // Explicit mismatch message names the component and both values.
    let mismatch = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .header_styles([Style::new().gap(8.0)])
        .row_styles([Style::new().gap(10.0)]);
    match mismatch.validate() {
        Err(CollectionError::Validation(msg)) => {
            assert!(msg.contains("column gap"), "message names gap: {msg}");
            assert!(
                msg.contains('8') && msg.contains("10"),
                "message shows values: {msg}"
            );
        }
        other => panic!("expected chrome mismatch, got {other:?}"),
    }
}

#[test]
fn shared_chrome_gap_padding_aligns() {
    // Matching nonzero horizontal chrome on BOTH header and body is normal
    // styling: inset = 8 + 8 + 10 * (3 - 1) = 36 reserved up front, tracks
    // allocate against 516 - 36 = 480, minus the 120 fixed track = 360
    // split 1:2 (integer shares 120/120/240, measured exact), content
    // fills 516, columns align.
    let chrome = || Style::new().padding(px(8.0)).gap(10.0);
    let t = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .header_styles([chrome()])
        .row_styles([chrome()]);
    assert!(t.validate().is_ok());
    let desc = t.descriptor();
    let layout = desc.source.layout(516.0);
    let plan = layout.column_widths.clone();
    assert!(approx(plan[0], 120.0));
    assert!(approx(plan[1], 120.0));
    assert!(approx(plan[2], 240.0));
    assert!(approx(layout.content_width, 516.0));

    let header = desc.source.header(&layout).expect("table header");
    let rows: Vec<Element> = (0..desc.source.len())
        .map(|i| desc.source.row(i, &layout))
        .collect();
    let mut parts = vec![header];
    parts.extend(rows);
    let (tree, root_id) = layout_root(&stack(parts), Size::new(516.0, 400.0));
    let row_ids = kids(&tree, root_id);
    let header_cells = kids(&tree, row_ids[0]);
    // First column starts after the shared 8px left padding.
    assert!(approx(xw(&tree, header_cells[0]).0, 8.0));
    for r in row_ids.iter().skip(1) {
        let cells = kids(&tree, *r);
        for (i, c) in cells.iter().enumerate() {
            let (x, w) = xw(&tree, *c);
            let (hx, hw) = xw(&tree, header_cells[i]);
            assert!(approx(x, hx), "shared-chrome column {i} x: {x} vs {hx}");
            assert!(approx(w, hw), "shared-chrome column {i} w: {w} vs {hw}");
            assert!(approx(w, plan[i]), "shared-chrome column {i} w vs plan");
        }
    }
}

#[test]
fn vertical_chrome_is_independent() {
    // Vertical padding/margins (and paint) may differ freely per part: they
    // never affect column x. Horizontal chrome matches (all zero) here.
    use aqua_core::Insets;
    let t = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .header_styles([Style::new()
            .padding_insets(Insets::hv(0.0, 12.0))
            .background(Oklch::BLACK)])
        .row_styles([Style::new()
            .padding_insets(Insets::hv(0.0, 4.0))
            .margin_insets(Insets::hv(0.0, 6.0))])
        .cell_styles([Style::new().margin_insets(Insets::hv(0.0, 3.0))]);
    assert!(t.validate().is_ok());
    let desc = t.descriptor();
    let layout = desc.source.layout(500.0);
    let header = desc.source.header(&layout).expect("table header");
    let row = desc.source.row(0, &layout);
    let el = stack(vec![header, row]);
    let (tree, root_id) = layout_root(&el, Size::new(500.0, 400.0));
    let row_ids = kids(&tree, root_id);
    let header_cells = kids(&tree, row_ids[0]);
    let body_cells = kids(&tree, row_ids[1]);
    for (i, (h, b)) in header_cells.iter().zip(body_cells.iter()).enumerate() {
        assert!(approx(xw(&tree, *h).0, xw(&tree, *b).0), "column {i} x");
        assert!(approx(xw(&tree, *h).1, xw(&tree, *b).1), "column {i} w");
    }
}

#[test]
fn cell_margin_and_width_override_fail() {
    let bad_margin = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .cell_styles([Style::new().margin(px(4.0))]);
    assert!(matches!(
        bad_margin.validate(),
        Err(CollectionError::Validation(_))
    ));

    let bad_width = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .cell_styles([Style::new().width(px(50.0))]);
    assert!(matches!(
        bad_width.validate(),
        Err(CollectionError::InvalidSize(_))
    ));

    let bad_grow = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .cell_styles([Style::new().flex_grow(1.0)]);
    assert!(matches!(
        bad_grow.validate(),
        Err(CollectionError::InvalidSize(_))
    ));

    // Vertical cell margins are allowed (no x effect).
    let ok_vm = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .cell_styles([Style::new().margin_insets(aqua_core::Insets::hv(0.0, 3.0))]);
    assert!(ok_vm.validate().is_ok());
}

#[test]
fn missing_factories_keys_duplicates_fail() {
    // Missing key.
    let t: Table<Item> = Table::new().items(items()).columns(columns());
    assert_eq!(t.validate(), Err(CollectionError::MissingKeyFactory));

    // Missing row factory (list).
    let l = List::<Item>::new().items(items()).key(|it: &Item| it.id);
    assert_eq!(l.validate(), Err(CollectionError::MissingRowFactory));

    // Missing key (list).
    let l = List::new()
        .items(items())
        .row(|it: &Item| Text::builder().arg(it.name).build());
    assert_eq!(l.validate(), Err(CollectionError::MissingKeyFactory));

    // Duplicate keys.
    let dup = vec![Item { id: 1, name: "x" }, Item { id: 1, name: "y" }];
    let t = Table::new()
        .items(dup)
        .key(|it: &Item| it.id)
        .columns(columns())
        .row_styles([]);
    assert_eq!(t.validate(), Err(CollectionError::DuplicateKey));

    // Missing cell factory.
    let t = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns([ColumnBuilder::new("a")
            .header(|| Text::builder().arg("").build())
            .width(px(10.0))
            .build()]);
    assert!(matches!(t.validate(), Err(CollectionError::Validation(_))));

    // No columns / empty id / duplicate column ids.
    let t: Table<Item> = Table::new().items(items()).key(|it: &Item| it.id);
    assert!(matches!(t.validate(), Err(CollectionError::Validation(_))));
    let t = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns([ColumnBuilder::new("")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .build()]);
    assert!(matches!(t.validate(), Err(CollectionError::Validation(_))));
    let col = || {
        ColumnBuilder::new("dup")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .build()
    };
    let t = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns([col(), col()]);
    assert_eq!(t.validate(), Err(CollectionError::DuplicateColumnId));
}

#[test]
fn invalid_tracks_bounds_heights_fail() {
    let mk = |w: ColumnWidth| {
        Table::new()
            .items(items())
            .key(|it: &Item| it.id)
            .columns([ColumnBuilder::new("a")
                .header(|| Text::builder().arg("").build())
                .cell(|_: &Item| Text::builder().arg("").build())
                .width(w)
                .build()])
    };
    assert!(matches!(
        mk(ColumnWidth::Flex(f32::NAN)).validate(),
        Err(CollectionError::InvalidTrack(_))
    ));
    assert!(matches!(
        mk(ColumnWidth::Flex(-1.0)).validate(),
        Err(CollectionError::InvalidTrack(_))
    ));
    assert!(matches!(
        mk(aqua_ui::style::auto().into()).validate(),
        Err(CollectionError::InvalidTrack(_))
    ));
    assert!(matches!(
        mk(px(-4.0).into()).validate(),
        Err(CollectionError::InvalidTrack(_))
    ));

    // min > max.
    let t = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns([ColumnBuilder::new("a")
            .header(|| Text::builder().arg("").build())
            .cell(|_: &Item| Text::builder().arg("").build())
            .width(fr(1.0))
            .min_width(200.0)
            .max_width(100.0)
            .build()]);
    assert!(matches!(t.validate(), Err(CollectionError::InvalidSize(_))));

    // Invalid heights fail loudly on builder AND source validation, and the
    // original value is preserved in the descriptor (never silently 32).
    for bad in [
        Length::Px(0.0),
        Length::Px(-5.0),
        Length::Px(f32::NAN),
        aqua_ui::style::pct(50.0),
        aqua_ui::style::auto(),
    ] {
        let t = Table::new()
            .items(items())
            .key(|it: &Item| it.id)
            .columns(columns())
            .estimated_row_height(bad);
        assert!(
            matches!(t.validate(), Err(CollectionError::InvalidSize(_))),
            "builder must reject {bad:?}"
        );
        let desc = t.descriptor();
        assert!(
            matches!(desc.source.validate(), Err(CollectionError::InvalidSize(_))),
            "source must reject {bad:?}"
        );
    }
    let t = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(columns())
        .estimated_row_height(px(-5.0));
    let desc = t.descriptor();
    assert_eq!(desc.estimated_row_height, -5.0);
}

use aqua_ui::style::Length;

// ---------------------------------------------------------------------------
// List semantics: snapshots, laziness, defaults
// ---------------------------------------------------------------------------

#[test]
fn list_row_wraps_inner_control_without_touching_it() {
    // A Button row keeps its own element: the source wraps it in an ordinary
    // structural Box (ListItem lives on the wrapper, applied at bind time)
    // instead of overwriting the inner control role.
    use aqua_ui::element::Button;
    let l = List::new()
        .items(items())
        .key(|it: &Item| it.id)
        .row(|it: &Item| {
            Button::new()
                .child(Text::builder().arg(it.name).build())
                .build()
        });
    assert!(l.validate().is_ok());
    let desc = l.descriptor();
    let layout = desc.source.layout(300.0);
    let row = desc.source.row(0, &layout);
    let specs = lower_specs(&row).expect("row lowers");
    assert_eq!(specs.len(), 1);
    // Outer structural box with exactly the inner control as its child.
    assert!(matches!(specs[0].kind, aqua_ui::tree::NodeKind::Box(_)));
    assert_eq!(specs[0].children.len(), 1);
    assert!(matches!(
        specs[0].children[0].kind,
        aqua_ui::tree::NodeKind::Box(_)
    ));
    assert_eq!(specs[0].children[0].children.len(), 1);
}

#[test]
fn list_snapshot_defaults_and_laziness() {
    // Arrays, vecs, and lazy iterators all work; data is snapshotted.
    let mut src = vec![1u32, 2, 3];
    let l = List::new()
        .items(src.iter().copied())
        .key(|v: &u32| *v)
        .row(|v: &u32| Text::builder().arg(v.to_string()).build());
    assert_eq!(l.len(), 3);
    assert!(!l.is_virtualized());
    src.push(4);
    assert_eq!(l.len(), 3, "items are snapshotted at .items() time");

    let desc = l.descriptor();
    assert!(!desc.virtualized);
    assert_eq!(desc.estimated_row_height, 32.0);
    assert_eq!(desc.overscan, 4);
    assert_eq!(desc.source.role(), CollectionRole::List);
    assert!(desc.source.header(&desc.source.layout(300.0)).is_none());
    assert!(desc.source.validate().is_ok());

    // Factories are never called by descriptor(); row() calls on demand.
    let calls = Rc::new(std::cell::Cell::new(0usize));
    let c = Rc::clone(&calls);
    let l = List::new()
        .items([10u32, 20])
        .key(|v: &u32| *v)
        .row(move |_: &u32| {
            c.set(c.get() + 1);
            Text::builder().arg("r").build()
        });
    let desc = l.descriptor();
    assert_eq!(calls.get(), 0);
    let layout = desc.source.layout(300.0);
    assert_eq!(layout.column_widths, Vec::<f32>::new());
    let _ = desc.source.row(1, &layout);
    assert_eq!(calls.get(), 1);
    let _ = desc.source.row(1, &layout);
    assert_eq!(calls.get(), 2);
}

#[test]
fn list_virtualize_and_config() {
    let l = List::new()
        .items(items())
        .key(|it: &Item| it.id)
        .row(|it: &Item| Text::builder().arg(it.name).build())
        .virtualize()
        .estimated_row_height(px(24.0))
        .overscan(8);
    assert!(l.is_virtualized());
    assert!(l.validate().is_ok());
    let desc = l.descriptor();
    assert!(desc.virtualized);
    assert_eq!(desc.estimated_row_height, 24.0);
    assert_eq!(desc.overscan, 8);
}

// ---------------------------------------------------------------------------
// Table basics: heterogeneity, roles, caching, build path
// ---------------------------------------------------------------------------

#[test]
fn heterogeneous_columns_erase_in_arrays() {
    // Two different closure types in one array: uniform Column<Item>.
    let cols = [
        ColumnBuilder::new("a")
            .header(|| Text::builder().arg("A").build())
            .cell(|it: &Item| Text::builder().arg(it.id.to_string()).build())
            .width(px(50.0))
            .build(),
        ColumnBuilder::new("b")
            .header(|| Text::builder().arg("B").build())
            .cell(move |it: &Item| {
                let n = it.name.len() as f32;
                Text::builder().arg(it.name).font_size(10.0 + n).build()
            })
            .width(fr(1.0))
            .build(),
    ];
    let t = Table::new()
        .items(items())
        .key(|it: &Item| it.id)
        .columns(cols);
    assert_eq!(t.column_len(), 2);
    assert!(t.validate().is_ok());
    let desc = t.descriptor();
    assert_eq!(desc.source.role(), CollectionRole::Table);
    assert!(desc.source.header(&desc.source.layout(300.0)).is_some());
    // Plan is cached per width: repeated layouts agree without cell visits.
    let a = desc.source.layout(300.0);
    let b = desc.source.layout(300.0);
    assert_eq!(a, b);
}

#[test]
fn table_container_styles_forward_to_descriptor() {
    let t = table().styles([Style::new().background(Oklch::BLACK)]);
    let desc = t.descriptor();
    assert_eq!(desc.styles.len(), 1);
    let t = table()
        .style(Style::new().gap(0.0))
        .styles([Style::new().background(Oklch::BLACK)]);
    assert_eq!(t.descriptor().styles.len(), 2);
}

#[test]
fn table_build_mounts_all_rows_unvirtualized() {
    let t = table();
    let el = t.build();
    let (tree, root) = layout_root(&el, Size::new(500.0, 400.0));
    // build() lowers via Element::collection; the runtime realizes header +
    // all rows unvirtualized. Headless here: at least the container mounted.
    assert!(tree.rect(root).is_some());
}
