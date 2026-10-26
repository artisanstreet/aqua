//! Facade factory tests: canonical `Aqua::` builders, iterator children,
//! and the `List`/`Table`/`Column` + `fr` vocabulary (single source:
//! `aqua_ui::collection`).
//!
//! All cases mount through the real `UiApp` layout/scene path (headless).
//! Virtualized behavior is covered by `collections_conformance.rs`.

use aqua::{fr, px, Aqua, Style, UiApp};
use aqua_core::Oklch;
use aqua_platform::geometry::LogicalSize;
use aqua_platform::{AquaWindowId, ObservedWindowState};
use aqua_render::Scene;
use aqua_ui::tree::NodeKind;

fn observed() -> ObservedWindowState {
    ObservedWindowState {
        id: AquaWindowId::new(1).expect("window id"),
        title: "factories".to_string(),
        inner_size: LogicalSize::new(800.0, 600.0),
        scale_factor: 1.0,
        focused: true,
        visible: true,
        occluded: false,
    }
}

fn host() -> UiApp {
    let fonts = aqua_text::SharedFonts::new();
    let runs = aqua_text::SharedRuns::new();
    UiApp::new(fonts, runs)
}

fn frame(app: &mut UiApp, scene: &mut Scene) {
    app.sync().expect("sync drains");
    app.build_scene_into(scene, &observed())
        .expect("frame builds");
    assert_eq!(app.build_error(), None);
}

fn texts(app: &UiApp) -> Vec<String> {
    app.tree()
        .traverse()
        .into_iter()
        .filter_map(|n| match app.tree().get(n).map(|r| &r.kind) {
            Some(NodeKind::Text(t)) if !t.is_editable() && !t.text().is_empty() => {
                Some(t.text().to_string())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn every_widget_factory_mounts() {
    let mut app = host();
    let image = aqua_core::ImageId::from_raw(aqua_core::RawId {
        index: 7,
        generation: 0,
    });
    let (sig, _set) = app.scope().signal(String::from("dyn"));
    app.mount(|_| {
        Aqua::Box()
            .child(Aqua::Text("hello").build())
            .child(Aqua::Button().child(Aqua::Text("go").build()).build())
            .child(Aqua::Scroll().child(Aqua::Text("scroll").build()).build())
            .child(Aqua::Canvas().build())
            .child(Aqua::Viewport3D().build())
            .child(Aqua::Checkbox("check").checked(true).build())
            .child(Aqua::Slider(0.5).build())
            .child(Aqua::Dropdown("open").build())
            .child(
                Aqua::Tabs()
                    .tabs(vec!["a".into(), "b".into()])
                    .selected(0)
                    .build(),
            )
            .child(Aqua::Fragment().child(Aqua::Text("frag").build()).build())
            .child(Aqua::Dynamic(sig.clone()).build())
            .child(Aqua::Media(image).build())
            .build()
    })
    .expect("mount factories");
    app.set_viewport(800.0, 600.0, 1.0);
    let mut scene = Scene::new();
    frame(&mut app, &mut scene);
    assert!(scene.ordered().count() > 0);
    let labels = texts(&app);
    for want in ["hello", "go", "scroll", "check", "open", "frag", "dyn"] {
        assert!(
            labels.iter().any(|t| t == want),
            "missing {want}: {labels:?}"
        );
    }
}

#[test]
fn text_factory_takes_literals_and_signals() {
    let mut app = host();
    let (sig, set_sig) = app.scope().signal(String::from("one"));
    app.mount(|_| {
        Aqua::Box()
            .child(Aqua::Text("literal").build())
            .child(Aqua::Text(sig.clone()).build())
            .build()
    })
    .unwrap();
    app.set_viewport(400.0, 300.0, 1.0);
    let mut scene = Scene::new();
    frame(&mut app, &mut scene);
    assert!(texts(&app).iter().any(|t| t == "one"));
    set_sig.set("two".to_string());
    frame(&mut app, &mut scene);
    assert!(texts(&app).iter().any(|t| t == "two"));
}

#[test]
fn array_vec_and_lazy_iter_children_keep_order() {
    let from_array = Aqua::Box()
        .children([Aqua::Text("a").build(), Aqua::Text("b").build()])
        .child(Aqua::Text("c").build())
        .build();
    let parts: Vec<_> = ["x", "y"]
        .into_iter()
        .map(|t| Aqua::Text(t).build())
        .collect();
    let from_vec = Aqua::Box().children(parts).build();
    let lazy = Aqua::Box()
        .children((0..5u64).map(|i| Aqua::Text(format!("row-{i}")).build().with_key(i)))
        .build();
    for (element, expected) in [
        (from_array, vec!["a".to_string(), "b".into(), "c".into()]),
        (from_vec, vec!["x".to_string(), "y".into()]),
        (lazy, (0..5).map(|i| format!("row-{i}")).collect()),
    ] {
        let mut app = host();
        app.mount(|_| element).unwrap();
        let mut scene = Scene::new();
        frame(&mut app, &mut scene);
        let labels = texts(&app);
        let positions: Vec<_> = expected
            .iter()
            .map(|w| labels.iter().position(|t| t == w).expect("child present"))
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] < w[1]),
            "children keep order: {labels:?}"
        );
    }
}

#[test]
fn app_config_window_style_factories_compose() {
    let app = Aqua::App().config(
        Aqua::Config().window(
            Aqua::Window()
                .title("factories")
                .size(px(760.0), px(560.0))
                .min_size(px(480.0), px(360.0)),
        ),
    );
    let _ = app;
    let style = Aqua::Style().width(px(120.0)).background(Oklch::WHITE);
    assert!(!style.is_empty());
    let composed = Style::compose(&[style]);
    assert_eq!(
        composed.resolve_layout().width,
        aqua_ui::style::Length::Px(120.0)
    );
}

#[test]
fn fr_is_a_table_track_not_a_length() {
    // `fr` must not extend `Length`: it builds the peer's distinct
    // `ColumnWidth` track type (single source: `aqua_ui::collection`).
    // Column widths accept both spellings; types prove the separation.
    fn assert_track(_: aqua::ColumnWidth) {}
    assert_track(fr(2.0));
    assert_track(px(120.0).into());
    let _: aqua::ColumnBuilder<u64> = Aqua::Column("c").width(px(80.0));
    let _: aqua::ColumnBuilder<u64> = Aqua::Column("c").width(fr(1.0));
}

#[test]
fn list_factory_builds_keyed_rows_through_ui_app() {
    #[derive(Clone)]
    struct Item(u64, String);
    let items: Vec<Item> = (0..32u64).map(|i| Item(i, format!("item-{i}"))).collect();
    // Unvirtualized default: mounts every row through the same row factories.
    let mut app = host();
    app.mount(|_| {
        Aqua::List()
            .items(items.iter().cloned())
            .key(|it: &Item| it.0)
            .row(|it: &Item| Aqua::Text(it.1.clone()).build())
            .styles([Style::new().gap(4.0)])
            .build()
    })
    .unwrap();
    app.set_viewport(600.0, 800.0, 1.0);
    let mut scene = Scene::new();
    frame(&mut app, &mut scene);
    let labels = texts(&app);
    assert_eq!(labels.iter().filter(|t| t.starts_with("item-")).count(), 32);
    assert_eq!(app.build_error(), None);
    // Virtualization opt-in: same call sites plus `.virtualize()`.
    let mut app = host();
    app.mount(|_| {
        Aqua::List()
            .items(items.iter().cloned())
            .key(|it: &Item| it.0)
            .row(|it: &Item| Aqua::Text(it.1.clone()).build())
            .virtualize()
            .build()
    })
    .unwrap();
    app.set_viewport(600.0, 800.0, 1.0);
    let mut scene = Scene::new();
    frame(&mut app, &mut scene);
    assert_eq!(app.build_error(), None);
}

#[test]
fn table_factory_shares_one_width_plan_with_real_headers() {
    #[derive(Clone)]
    struct Item(u64, String, u64);
    let items: Vec<Item> = (0..16u64)
        .map(|i| Item(i, format!("name-{i}"), 100 - i))
        .collect();
    let mut app = host();
    app.mount(|_| {
        Aqua::Table()
            .items(items.iter().cloned())
            .key(|it: &Item| it.0)
            .columns([
                Aqua::Column("name")
                    .header(|| Aqua::Text("Name").build())
                    .cell(|it: &Item| Aqua::Text(it.1.clone()).build())
                    .width(fr(2.0))
                    .build(),
                Aqua::Column("value")
                    .header(|| Aqua::Text("Value").build())
                    .cell(|it: &Item| Aqua::Text(format!("{}", it.2)).build())
                    .width(px(90.0))
                    .build(),
            ])
            .header_styles([Style::new().gap(4.0)])
            .row_styles([Style::new().gap(4.0)])
            .virtualize()
            .build()
    })
    .unwrap();
    app.set_viewport(700.0, 600.0, 1.0);
    let mut scene = Scene::new();
    frame(&mut app, &mut scene);
    let labels = texts(&app);
    // Real header text (not fake controls) plus body cells.
    assert!(labels.iter().any(|t| t == "Name"), "{labels:?}");
    assert!(labels.iter().any(|t| t == "Value"), "{labels:?}");
    assert!(labels.iter().any(|t| t == "name-0"), "{labels:?}");
    assert_eq!(app.build_error(), None);
}
