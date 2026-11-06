#![cfg(windows)]

use aqua::render::lower::Lowering;
use aqua_core::scene::DisplayItem;
use aqua_core::{Size, Transform2D};
use aqua_graph::PaintCommand;
use aqua_protocol::Instant;
use aqua_runtime::Runtime;
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::style::{px, Axis, Style};

const VIEWPORT: Size = Size { w: 640.0, h: 480.0 };

fn transcript(runtime: &mut Runtime) {
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column).width(px(600.0)))
                .children((0..30).map(|index| {
                    UiText::builder()
                        .arg(format!(
                            "Row {index}. Retained text lowers once and is re-placed when the content moves under a scroll glide."
                        ))
                        .wrap(true)
                        .style(Style::new().width(px(560.0)))
                        .build()
                }))
                .build()
        })
        .unwrap();
}

fn placement(commands: &[PaintCommand]) -> Vec<String> {
    commands
        .iter()
        .map(|command| match command {
            PaintCommand::Glyphs(run) => format!(
                "glyphs {:?}",
                run.glyphs
                    .iter()
                    .map(|glyph| (glyph.bounds, glyph.color))
                    .collect::<Vec<_>>()
            ),
            PaintCommand::Image(image) => format!("image {:?}", image.bounds),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn replaced_runs_equal_fresh_lowering_at_every_offset() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut runtime = Runtime::new(fonts.clone(), runs.clone());
    runtime.set_viewport(VIEWPORT.w, VIEWPORT.h, 1.25);
    transcript(&mut runtime);
    runtime.build_frame().unwrap();

    let mut scene = Scene::new();
    let glide = scene.insert(DisplayItem::PushTransform(Transform2D::IDENTITY));
    for (_, item) in runtime.scene().ordered() {
        scene.insert(item.clone());
    }
    scene.insert(DisplayItem::PopTransform);

    let mut warm = Lowering::new(fonts.clone(), runs.clone());
    let offsets = [
        (0.0, 0.0),
        (0.0, 0.3),
        (0.0, 0.3),
        (0.0, 3.7),
        (0.0, 17.45),
        (1.0, 17.45),
        (1.0, 18.45),
        (0.25, -5.6),
        (0.0, 0.0),
    ];
    for (dx, dy) in offsets {
        scene.update(
            glide,
            DisplayItem::PushTransform(Transform2D::translate(dx, dy)),
        );
        let (cached, _, cached_report) = warm.commands(&scene, VIEWPORT, 1.25, Instant::ZERO);
        let mut fresh = Lowering::new(fonts.clone(), runs.clone());
        let (expected, _, expected_report) = fresh.commands(&scene, VIEWPORT, 1.25, Instant::ZERO);
        assert!(expected_report.glyphs > 0, "text must lower to glyphs");
        assert_eq!(cached_report.glyphs, expected_report.glyphs);
        assert_eq!(
            placement(&cached),
            placement(&expected),
            "offset ({dx}, {dy})"
        );
        warm.retain_frame(cached);
    }
    assert!(warm.span_replays() >= 5);
}
