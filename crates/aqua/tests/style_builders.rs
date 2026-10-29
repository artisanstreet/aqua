//! Facade-level `Style` builder tests over the public `aqua` surface.
//!
//! Covers: explicit unset vs override, later-wins merge, repeated builder
//! `.styles` calls mixed with direct setters, helper `const` sharing,
//! no wrapper nodes around painted buttons, child text inheriting parent typography
//! through a real mount, keyed refresh retaining node identity, focus,
//! and input, and a `const`-position compile proof.

use aqua::prelude::*;
use aqua::ui::{Box as UiBox, Text as UiText};
use aqua::{auto, pct, px, Style, UiApp};
use aqua_core::{Insets, NodeId};
use aqua_platform::geometry::LogicalSize;
use aqua_platform::{AquaWindowId, ObservedWindowState};
use aqua_render::Scene;
use aqua_text::{SharedFonts, SharedRuns};

// Shared const helpers: scalar const chains, const Oklch palette.
const INK: Oklch = Oklch::new(0.19912866, 0.022232028, 260.4666); // #101620
const PAPER: Oklch = Oklch::WHITE;
const MINT: Oklch = Oklch::new(0.8972348, 0.08736119, 164.26053); // #a6f0ce
const BASE: Style = Style::new()
    .width(px(200.0))
    .padding(px(8.0))
    .radius(px(6.0))
    .background(PAPER)
    .font_size(14.0)
    .text_color(INK);
const OVERRIDE: Style = Style::new().width(px(120.0)).text_color(PAPER);

// Compile proof: const chains over the frozen API, including per-edge
// insets and clearing to explicit-none.
const PROOF: Style = Style::new()
    .width(pct(50.0))
    .height(auto())
    .padding_insets(Insets {
        top: 1.0,
        bottom: 2.0,
        left: 3.0,
        right: 4.0,
    })
    .margin(px(4.0))
    .clear_border();

fn fonts_and_runs() -> (SharedFonts, SharedRuns) {
    (SharedFonts::new(), SharedRuns::new())
}

fn observed(w: f64, h: f64) -> ObservedWindowState {
    ObservedWindowState {
        id: AquaWindowId::new(1).unwrap(),
        title: "style-builders-test".to_string(),
        inner_size: LogicalSize::new(w, h),
        scale_factor: 1.0,
        focused: true,
        visible: true,
        occluded: false,
    }
}

/// Every text node id in traversal order with its committed text.
fn text_nodes(app: &UiApp) -> Vec<(NodeId, String)> {
    app.tree()
        .traverse()
        .into_iter()
        .filter_map(|n| match app.tree().get(n).map(|r| &r.kind) {
            Some(aqua_ui::tree::NodeKind::Text(t)) => Some((n, t.text().to_string())),
            _ => None,
        })
        .collect()
}

#[test]
fn unset_leaves_target_alone() {
    let mut l = aqua_ui::style::LayoutStyle::default();
    let mut p = aqua_ui::style::Paint::default();
    Style::new().apply_to(&mut l, &mut p);
    assert_eq!(l, aqua_ui::style::LayoutStyle::default());
    assert_eq!(p, aqua_ui::style::Paint::default());
    assert!(Style::new().is_empty());
    assert!(!BASE.is_empty());
}

#[test]
fn later_wins_per_field_only() {
    let s = Style::compose(&[BASE, OVERRIDE]);
    let layout = s.resolve_layout();
    assert_eq!(layout.width, aqua::Length::Px(120.0));
    // Untouched by OVERRIDE: BASE padding survives.
    assert_eq!(
        layout.padding,
        Insets {
            top: 8.0,
            bottom: 8.0,
            left: 8.0,
            right: 8.0
        }
    );
    assert_eq!(s.resolve_paint().radius, 6.0);
    assert_eq!(s.text_color_opt(), Some(PAPER));
    assert_eq!(s.font_size_opt(), Some(14.0));
}

#[test]
fn explicit_none_overrides_value() {
    let s = Style::compose(&[BASE, Style::new().clear_background()]);
    assert_eq!(s.resolve_paint().fill, None);
}

#[test]
fn repeated_builder_styles_calls_mix_direct_setters() {
    // Real repeated `.styles` calls interleaved with direct setters: every
    // call merges in order, later set fields winning regardless of form.
    let el = UiBox::builder()
        .width(200)
        .styles([BASE])
        .width(50)
        .styles([OVERRIDE])
        .gap(4)
        .styles([Style::new().radius(px(2.0))])
        .build();
    let specs = aqua_ui::element::lower_specs(&el).unwrap();
    assert_eq!(specs.len(), 1);
    // OVERRIDE's width (120) beats the earlier direct `.width(50)`.
    assert_eq!(specs[0].style.width, aqua::Length::Px(120.0));
    // Direct `.gap(4)` survives: no style entry sets it.
    assert_eq!(specs[0].style.gap, 4.0);
    // Last radius entry wins over BASE's 6.0.
    let paint = match &specs[0].kind {
        aqua_ui::tree::NodeKind::Box(b) => b.paint,
        other => panic!("expected Box, got {other:?}"),
    };
    assert_eq!(paint.radius, 2.0);

    // The other direction: a direct setter after `.styles` wins.
    let el = UiBox::builder().styles([OVERRIDE]).width(77).build();
    let specs = aqua_ui::element::lower_specs(&el).unwrap();
    assert_eq!(specs[0].style.width, aqua::Length::Px(77.0));
    // And a legacy `style:` single still merges in call order.
    let el = UiBox::builder().styles([BASE]).style(OVERRIDE).build();
    let specs = aqua_ui::element::lower_specs(&el).unwrap();
    assert_eq!(specs[0].style.width, aqua::Length::Px(120.0));
}

#[test]
fn const_proof_resolves() {
    let layout = PROOF.resolve_layout();
    assert_eq!(layout.width, aqua::Length::Pct(50.0));
    assert_eq!(layout.height, aqua::Length::Auto);
    assert_eq!(
        layout.padding,
        Insets {
            top: 1.0,
            bottom: 2.0,
            left: 3.0,
            right: 4.0
        }
    );
    assert_eq!(
        layout.margin,
        Insets {
            top: 4.0,
            bottom: 4.0,
            left: 4.0,
            right: 4.0
        }
    );
    assert_eq!(PROOF.resolve_paint().border, None);
}

#[test]
fn builders_accept_shared_style_arrays() {
    // Direct builder form: shared consts composed per use, no wrappers.
    let el = UiBox::builder()
        .styles([BASE, OVERRIDE])
        .child(UiText::builder().arg("hi").styles([BASE]).build())
        .build();
    let specs = aqua_ui::element::lower_specs(&el).unwrap();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].style.width, aqua::Length::Px(120.0));
    assert_eq!(specs[0].children.len(), 1);
}

#[test]
fn button_paint_needs_no_wrapper_box() {
    // Painted button lowers to a single Box node carrying the fill.
    let el = UiBox::builder()
        .child(
            aqua::ui::Button::builder()
                .styles([BASE])
                .on_activate(|| {})
                .child(UiText::builder().arg("Go").build())
                .build(),
        )
        .build();
    let specs = aqua_ui::element::lower_specs(&el).unwrap();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].children.len(), 1);
    match &specs[0].children[0].kind {
        aqua_ui::tree::NodeKind::Box(b) => {
            assert_eq!(b.paint.fill, Some(PAPER));
            assert_eq!(b.paint.radius, 6.0);
        }
        other => panic!("expected Box, got {other:?}"),
    }
}

#[test]
fn style_typography_defaults_resolve() {
    let typo = Style::compose(&[BASE, OVERRIDE]).resolve_typography();
    assert_eq!(typo.font_size, 14.0);
    assert_eq!(typo.color, PAPER);
    assert!(!typo.wrap);
    let wrapped = Style::new().text_wrap(true).resolve_typography();
    assert!(wrapped.wrap);
}

#[test]
fn child_text_inherits_parent_typography_through_mount() {
    let (fonts, runs) = fonts_and_runs();
    let mut app = UiApp::new(fonts, runs);
    app.mount(|_scope| {
        UiBox::builder()
            .styles([Style::new().text_color(MINT).font_size(20.0)])
            .child(UiText::builder().arg("plain").build())
            .child(UiText::builder().arg("own").color(PAPER).build())
            .build()
    })
    .unwrap();
    app.sync().unwrap();
    let nodes = text_nodes(&app);
    assert_eq!(nodes.len(), 2);
    let plain = nodes
        .iter()
        .find(|(_, t)| t == "plain")
        .expect("plain label mounted");
    let own = nodes
        .iter()
        .find(|(_, t)| t == "own")
        .expect("own-color label mounted");
    let content = |id: NodeId| match &app.tree().get(id).unwrap().kind {
        aqua_ui::tree::NodeKind::Text(t) => (t.color, t.font_size),
        _ => panic!("expected text"),
    };
    // Genuine inheritance: the bare child renders the parent's color and
    // size; the explicit `.color` wins for color but still inherits size.
    assert_eq!(content(plain.0), (MINT, 20.0));
    assert_eq!(content(own.0), (PAPER, 20.0));
    assert_eq!(app.build_error(), None);
}

#[test]
fn keyed_refresh_retains_node_focus_and_input() {
    let (fonts, runs) = fonts_and_runs();
    let mut app = UiApp::new(fonts, runs);
    let (read, write) = app.scope().signal("draft".to_string());
    let (r2, w2) = (read.clone(), write.clone());
    let (r3, w3) = (read.clone(), write.clone());
    app.mount(|_scope| {
        UiBox::builder()
            .child(
                UiText::builder()
                    .bind((r2, w2))
                    .styles([BASE])
                    .build()
                    .with_key("field"),
            )
            .child(UiText::builder().arg("row-a").build().with_key("a"))
            .build()
    })
    .unwrap();
    app.sync().unwrap();

    // Focus the bound field and type through the real input path.
    let field = text_nodes(&app)
        .into_iter()
        .find(|(_, t)| t == "draft")
        .map(|(id, _)| id)
        .expect("bound field mounted");
    app.tree_mut().set_focusable(field, true);
    app.tree_mut().set_focused(Some(field));
    assert!(app.handle_input(&InputEvent::TextInput {
        text: "!".to_string()
    }));
    app.sync().unwrap();
    let typed = text_nodes(&app)
        .into_iter()
        .find(|(id, _)| *id == field)
        .map(|(_, t)| t)
        .expect("field still mounted");
    assert_ne!(typed, "draft", "keystroke lands in the bound editor");

    // Structural refresh: same keys (restyled), order changed, row added.
    app.refresh(vec![UiBox::builder()
        .child(UiText::builder().arg("row-a").build().with_key("a"))
        .child(UiText::builder().arg("row-b").build().with_key("b"))
        .child(
            UiText::builder()
                .bind((r3, w3))
                .styles([BASE, OVERRIDE])
                .build()
                .with_key("field"),
        )
        .build()])
        .unwrap();
    app.sync().unwrap();
    assert_eq!(app.build_error(), None);

    // Identity: the keyed field is the same node, focus followed it, the
    // typed input survived the refresh, and the new styles applied to the
    // retained node (width now comes from OVERRIDE).
    assert_eq!(app.tree().focused(), Some(field));
    let after = text_nodes(&app);
    let field_text = after
        .iter()
        .find(|(id, _)| *id == field)
        .expect("keyed field node retained");
    assert_eq!(field_text.1, typed);
    assert!(after.iter().any(|(_, t)| t == "row-b"));
    assert_eq!(
        app.tree().get(field).unwrap().style.width,
        aqua::Length::Px(120.0),
        "restyle applies in place on the retained node"
    );
    let mut scene = Scene::new();
    app.build_scene_into(&mut scene, &observed(800.0, 600.0))
        .unwrap();
    assert!(!scene.is_empty());
}
