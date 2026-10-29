//! Collection demo: a 100k-row virtualized list and a sortable table.
//!
//! Builder-first through the canonical factories: `Aqua::List()` /
//! `Aqua::Table()` / `Aqua::Column(..)` with `fr(..)` tracks. Both collections
//! opt in to virtualization (`.virtualize()`; OFF by default) and carry real
//! styles. Table headers are real elements — the sortable column header is a
//! real `Button` that re-sorts application-owned state, never a fake control.
//!
//! - Windowed (default): `cargo run -p aqua --example collections`
//! - Headless (CI): `cargo run -p aqua --example collections -- --headless`
//! - Offscreen PNG: `cargo run -p aqua --example collections -- --preview out.png`
//!
//! Every path mounts the real 100,000-row descriptor through `UiApp`. Both
//! collections carry finite viewport heights (list 220, table 260 logical px
//! inside the 900x700 window), so virtualization bounds the mounted window;
//! the pumps assert the mounted row count stays bounded while the logical
//! total (100,000) is reported through the header.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use aqua::config::{Material, Theme, Titlebar};
use aqua::prelude::*;
use aqua::{px, Aqua, Context, IntoView, Style, UiApp};
use aqua_core::Oklch;
use taffy::style::{AlignItems, JustifyContent};

const TOTAL_ROWS: usize = 100_000;
/// Upper bound for mounted rows in the 220/260px viewports (generous over the
/// runtime's overscan window; failure means virtualization did not bound).
const MOUNTED_BOUND: usize = 2_000;

#[derive(Clone, Debug, PartialEq)]
struct Row {
    id: u64,
    name: String,
    value: u64,
}

fn make_rows(n: usize) -> Vec<Row> {
    (0..n as u64)
        .map(|i| Row {
            id: i,
            name: format!("row-{i:06}"),
            value: (i * 2654435761) % 10_007,
        })
        .collect()
}

const PAGE: Style = Aqua::Style()
    .width(aqua::pct(100.0))
    .height(aqua::pct(100.0))
    .column()
    .align(AlignItems::CENTER)
    .padding_insets(aqua_core::Insets {
        top: 64.0,
        right: 24.0,
        bottom: 24.0,
        left: 24.0,
    })
    .background(Oklch::new(0.21, 0.006, 260.0));
const CONTENT: Style = Aqua::Style()
    .width(aqua::pct(100.0))
    .max_width(px(900.0))
    .column()
    .gap(16.0);
const HEAD: Style = Aqua::Style()
    .row()
    .align(AlignItems::CENTER)
    .justify(JustifyContent::SPACE_BETWEEN)
    .width(aqua::pct(100.0));
const TITLE: Style = Aqua::Style()
    .font_size(24.0)
    .text_color(Oklch::WHITE.with_alpha(0.93));
const SUB: Style = Aqua::Style()
    .font_size(12.0)
    .text_color(Oklch::WHITE.with_alpha(0.56));
// Finite viewport heights: virtualization can bound the mounted window only
// when the collection cannot expand to fit all rows.
const LIST_BOX: Style = Aqua::Style()
    .column()
    .gap(4.0)
    .width(aqua::pct(100.0))
    .height(px(220.0));
const TABLE_BOX: Style = Aqua::Style()
    .column()
    .width(aqua::pct(100.0))
    .height(px(260.0));
const ROW_BOX: Style = Aqua::Style()
    .width(aqua::pct(100.0))
    .row()
    .align(AlignItems::CENTER)
    .gap(10.0)
    .padding(px(8.0))
    .radius(px(8.0))
    .background(Oklch::WHITE.with_alpha(0.045));
const CELL_NAME: Style = Aqua::Style()
    .font_size(13.0)
    .text_color(Oklch::WHITE.with_alpha(0.9));
const CELL_VALUE: Style = Aqua::Style()
    .font_size(13.0)
    .text_color(Oklch::WHITE.with_alpha(0.6));
const HEADER_ROW: Style = Aqua::Style()
    .row()
    .gap(10.0)
    .padding(px(8.0))
    .background(Oklch::WHITE.with_alpha(0.07))
    .radius(px(8.0));
const HEADER_TX: Style = Aqua::Style()
    .font_size(12.0)
    .text_color(Oklch::WHITE.with_alpha(0.75));

fn row_element(r: &Row) -> aqua_ui::element::Element {
    Aqua::Box()
        .styles([ROW_BOX])
        .child(Aqua::Text(r.name.clone()).styles([CELL_NAME]).build())
        .child(
            Aqua::Text(format!("{}", r.value))
                .styles([CELL_VALUE])
                .build(),
        )
        .build()
}

/// Full 100k-row virtualized list element.
fn list_element(rows: &[Row]) -> aqua_ui::element::Element {
    Aqua::List()
        .items(rows.iter().cloned())
        .key(|r: &Row| r.id)
        .row(row_element)
        .styles([LIST_BOX])
        .virtualize()
        .build()
}

/// Sortable table element over `rows`. `ascending` is application-owned state;
/// the "Value" header is a real Button that flips it through the writer.
fn table_element(
    rows: &[Row],
    ascending: bool,
    sort_w: Option<aqua_reactive::WriteSignal<bool>>,
) -> aqua_ui::element::Element {
    let mut sorted: Vec<Row> = rows.to_vec();
    sorted.sort_by_key(|r| r.value);
    if !ascending {
        sorted.reverse();
    }
    let name_col = Aqua::Column("name")
        .header(|| Aqua::Text("Name").styles([HEADER_TX]).build())
        .cell(|r: &Row| Aqua::Text(r.name.clone()).styles([CELL_NAME]).build())
        .width(aqua::fr(2.0))
        .build();
    let value_header_label = if ascending { "Value ▲" } else { "Value ▼" };
    let value_header = move || match &sort_w {
        Some(w) => {
            let w = w.clone();
            Aqua::Button()
                .on_activate(move || w.update(|v| *v = !*v))
                .child(Aqua::Text(value_header_label).styles([HEADER_TX]).build())
                .build()
        }
        None => Aqua::Text(value_header_label).styles([HEADER_TX]).build(),
    };
    let value_col = Aqua::Column("value")
        .header(value_header)
        .cell(|r: &Row| {
            Aqua::Text(format!("{}", r.value))
                .styles([CELL_VALUE])
                .build()
        })
        .width(aqua::fr(1.0))
        .min_width(90)
        .build();
    Aqua::Table()
        .items(sorted)
        .key(|r: &Row| r.id)
        .columns([name_col, value_col])
        .header_styles([HEADER_ROW])
        .row_styles([ROW_BOX])
        .styles([TABLE_BOX])
        .virtualize()
        .build()
}

fn page_element(
    rows: &[Row],
    ascending: bool,
    sort_w: Option<aqua_reactive::WriteSignal<bool>>,
) -> aqua_ui::element::Element {
    Aqua::Box()
        .styles([PAGE])
        .child(
            Aqua::Box()
                .styles([CONTENT])
                .child(
                    Aqua::Box()
                        .styles([HEAD])
                        .child(Aqua::Text("Collections").styles([TITLE]).build())
                        .child(
                            Aqua::Text(format!("{TOTAL_ROWS} rows, virtualized"))
                                .styles([SUB])
                                .build(),
                        )
                        .build(),
                )
                .child(list_element(rows))
                .child(table_element(rows, ascending, sort_w))
                .build(),
        )
        .build()
}

/// Count mounted row labels (`row-XXXXXX`) in the live tree: bounded under
/// virtualization, never the logical total.
fn mounted_rows(app: &UiApp) -> usize {
    use aqua_ui::tree::NodeKind;
    app.tree()
        .traverse()
        .into_iter()
        .filter(|n| {
            matches!(app.tree().get(*n).map(|r| &r.kind),
                Some(NodeKind::Text(t)) if t.text().starts_with("row-"))
        })
        .count()
}

/// Deterministic headless pump: mount the real 100k-row descriptor through
/// the real UiApp path and assert virtualization bounds the mounted window.
fn headless_demo() -> Result<(), String> {
    let rows = make_rows(TOTAL_ROWS);

    let t0 = std::time::Instant::now();
    let element = page_element(&rows, true, None);
    let build_ms = t0.elapsed().as_millis();
    println!("built 100k-row list + 100k-row table descriptor in {build_ms}ms");

    let aqua = Aqua::new(AquaConfig::default())?;
    let mut app = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
    app.mount(|_| element)
        .map_err(|e| format!("mount failed: {e}"))?;
    app.set_viewport(900.0, 700.0, 1.0);
    app.sync().map_err(|e| e.to_string())?;
    app.build_frame().map_err(|e| e.to_string())?;
    let scene = app.scene();
    let mounted = mounted_rows(&app);
    println!(
        "mounted: logical={TOTAL_ROWS} mounted_rows={mounted} scene_ops={} text_runs={} build_error={:?}",
        scene.ordered().count(),
        app.text_run_count(),
        app.build_error()
    );
    assert!(
        scene.ordered().count() > 0,
        "mounted content emits scene ops"
    );
    assert!(
        mounted < MOUNTED_BOUND,
        "virtualization must bound mounted rows: {mounted} >= {MOUNTED_BOUND}"
    );
    assert!(
        mounted > 0,
        "viewport must realize visible rows (finite 220/260px heights)"
    );
    assert_eq!(app.build_error(), None);
    Ok(())
}

fn preview(path: &str) -> Result<(), String> {
    let rows = make_rows(TOTAL_ROWS);
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut app = UiApp::new(fonts.clone(), runs.clone());
    app.mount(|_| page_element(&rows, true, None))
        .map_err(|e| e.to_string())?;
    app.set_viewport(900.0, 700.0, 1.0);
    app.build_frame().map_err(|e| e.to_string())?;
    let mounted = mounted_rows(&app);
    println!("preview: logical={TOTAL_ROWS} mounted_rows={mounted}");
    assert!(
        mounted < MOUNTED_BOUND,
        "virtualization must bound mounted rows: {mounted} >= {MOUNTED_BOUND}"
    );
    let mut renderer =
        pollster::block_on(aqua_render::Renderer::headless(900, 700)).map_err(|e| e.to_string())?;
    renderer.set_text_provider(Box::new(aqua_render::GlyphonProvider::new(
        renderer.device(),
        renderer.queue(),
        renderer.format(),
        aqua_render::TextBackend::new(fonts, runs),
    )));
    let plan = renderer.prepare(app.scene_mut());
    renderer.draw(&plan).map_err(|e| e.to_string())?;
    let mut pixels = renderer.readback().map_err(|e| e.to_string())?;
    for rgba in pixels.as_chunks_mut::<4>().0 {
        for channel in &mut rgba[..3] {
            let x = f32::from(*channel) / 255.0;
            let srgb = if x <= 0.0031308 {
                x * 12.92
            } else {
                1.055 * x.powf(1.0 / 2.4) - 0.055
            };
            *channel = (srgb.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    image::RgbaImage::from_raw(900, 700, pixels)
        .unwrap()
        .save(path)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn collections_app(cx: &Context) -> impl IntoView {
    let (asc_r, asc_w) = cx.scope().signal(true);
    let rows = make_rows(TOTAL_ROWS);
    cx.view((asc_r,), move |(ascending,)| {
        page_element(&rows, ascending, Some(asc_w.clone()))
    })
}

fn main() -> Result<(), aqua::Error> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--preview") {
        let path = args
            .get(index + 1)
            .ok_or_else(|| aqua::Error::Init("--preview requires a PNG path".to_string()))?;
        return preview(path).map_err(aqua::Error::Init);
    }
    if std::env::args().any(|a| a == "--headless") {
        return headless_demo().map_err(aqua::Error::Init);
    }
    Aqua::App()
        .config(
            Aqua::Config().window(
                Aqua::Window()
                    .title("Aqua — Collections")
                    .size(px(900.0), px(700.0))
                    .min_size(px(480.0), px(360.0))
                    .theme(Theme::Dark)
                    .material(Material::Glass)
                    .titlebar(Titlebar::Overlay),
            ),
        )
        .run(collections_app)
}
