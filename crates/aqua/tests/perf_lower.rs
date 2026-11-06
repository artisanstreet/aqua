use std::time::Instant;

use aqua::render::lower::Lowering;
use aqua_core::geometry::{CornerRadii, Insets, Rect};
use aqua_core::scene::{DisplayItem, ShadowLayer};
use aqua_core::{Oklch, Size, Transform2D};
use aqua_graph::{plan_paint_with, Extent, PlanLimits};
use aqua_protocol::Instant as ProtoInstant;
use aqua_runtime::Runtime;
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::style::{px, Axis, Style};

const PARAGRAPH: &str = "Aqua keeps retained text, layout and scene state between frames so that an unchanged transcript costs nothing, while a streaming message only reshapes the run that changed.";
const VIEWPORT: Size = Size {
    w: 1518.0,
    h: 900.0,
};
const SCALE: f32 = 1.25;

fn color(r: u8, g: u8, b: u8, a: f32) -> Oklch {
    Oklch::from_srgb8(r, g, b, a)
}

fn mount_transcript(runtime: &mut Runtime, rows: usize) {
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column).width(px(900.0)))
                .children((0..rows).map(|index| {
                    UiBox::new()
                        .style(Style::new().width(px(900.0)).padding(px(8.0)))
                        .child(
                            UiText::builder()
                                .arg(format!(
                                    "Message {index}. {}",
                                    PARAGRAPH.repeat(1 + index % 3)
                                ))
                                .wrap(true)
                                .style(Style::new().width(px(860.0)))
                                .build(),
                        )
                        .build()
                }))
                .build()
        })
        .unwrap();
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn quad(bounds: Rect, radius: f32, fill: Oklch) -> DisplayItem {
    DisplayItem::Quad {
        bounds,
        corners: CornerRadii::uniform(radius),
        fill,
        border_widths: Insets::uniform(1.0),
        border_color: color(80, 80, 90, 0.6),
        dashed: false,
    }
}

fn shadow(bounds: Rect, radius: f32) -> DisplayItem {
    DisplayItem::Shadow {
        bounds,
        corners: CornerRadii::uniform(radius),
        layer: ShadowLayer {
            dx: 0.0,
            dy: 8.0,
            blur: 24.0,
            spread: 0.0,
            inset: false,
            color: color(0, 0, 0, 0.4),
        },
    }
}

fn decorations(scene: &mut Scene, glass: bool, group: bool) {
    for column in 0..(if glass { 2 } else { 0 }) {
        let x = 40.0 + column as f32 * 700.0;
        scene.insert(shadow(rect(x, 700.0, 640.0, 120.0), 14.0));
        scene.insert(DisplayItem::PushRoundedClip {
            bounds: rect(x, 700.0, 640.0, 120.0),
            corners: CornerRadii::uniform(14.0),
        });
        scene.insert(DisplayItem::BackdropBlurRounded {
            bounds: rect(x, 700.0, 640.0, 120.0),
            radius: 20.0,
            corners: CornerRadii::uniform(14.0),
        });
        scene.insert(quad(
            rect(x, 700.0, 640.0, 120.0),
            14.0,
            color(30, 30, 40, 0.55),
        ));
        scene.insert(DisplayItem::PopClip);
    }
    if !group {
        return;
    }
    scene.insert(DisplayItem::PushOpacity(0.92));
    scene.insert(shadow(rect(300.0, 100.0, 400.0, 300.0), 12.0));
    scene.insert(quad(
        rect(300.0, 100.0, 400.0, 300.0),
        12.0,
        color(40, 40, 50, 1.0),
    ));
    scene.insert(quad(
        rect(320.0, 120.0, 360.0, 260.0),
        8.0,
        color(60, 60, 70, 0.8),
    ));
    scene.insert(DisplayItem::PopOpacity);
}

fn percentile(sorted: &[f64], at: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * at) as usize]
}

fn report(label: &str, mut micros: Vec<f64>) {
    micros.sort_by(f64::total_cmp);
    println!(
        "perf-lower {label} p50_us={:.0} p90_us={:.0} p99_us={:.0} max_us={:.0}",
        percentile(&micros, 0.5),
        percentile(&micros, 0.9),
        percentile(&micros, 0.99),
        micros[micros.len() - 1],
    );
}

#[test]
#[ignore]
fn scroll_glide_lower_and_plan() {
    glide(true, true, true);
}

#[test]
#[ignore]
fn scroll_glide_without_decorations() {
    glide(false, false, true);
}

#[test]
#[ignore]
fn scroll_glide_without_chips() {
    glide(true, true, false);
}

#[test]
#[ignore]
fn scroll_glide_glass_only() {
    glide(true, false, true);
}

#[test]
#[ignore]
fn scroll_glide_group_only() {
    glide(false, true, true);
}

fn glide(glass: bool, group: bool, with_chips: bool) {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut runtime = Runtime::new(fonts.clone(), runs.clone());
    runtime.set_viewport(VIEWPORT.w, VIEWPORT.h, SCALE);
    mount_transcript(&mut runtime, 9);
    runtime.build_frame().unwrap();

    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushClip {
        bounds: rect(0.0, 0.0, VIEWPORT.w, VIEWPORT.h),
        radius: 0.0,
    });
    let glide = scene.insert(DisplayItem::PushTransform(Transform2D::IDENTITY));
    for (_, item) in runtime.scene().ordered() {
        scene.insert(item.clone());
    }
    for index in 0..(if with_chips { 220 } else { 0 }) {
        let y = (index as f32) * 4.0;
        let x = 940.0 + (index % 5) as f32 * 110.0;
        scene.insert(quad(rect(x, y, 100.0, 24.0), 6.0, color(50, 50, 60, 1.0)));
        if index % 11 == 0 {
            scene.insert(shadow(rect(x, y, 100.0, 24.0), 6.0));
        }
        if index % 7 == 0 {
            scene.insert(DisplayItem::PushClip {
                bounds: rect(x, y, 100.0, 24.0),
                radius: 6.0,
            });
            scene.insert(quad(
                rect(x + 4.0, y + 4.0, 60.0, 16.0),
                3.0,
                color(90, 90, 120, 1.0),
            ));
            scene.insert(DisplayItem::PopClip);
        }
    }
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopClip);
    decorations(&mut scene, glass, group);

    let mut lowering = Lowering::new(fonts, runs);
    let extent = Extent::new(
        (VIEWPORT.w * SCALE).ceil() as u32,
        (VIEWPORT.h * SCALE).ceil() as u32,
    );
    let clear = color(20, 20, 24, 1.0);
    let limits = PlanLimits::default();
    let mut lower_us = Vec::new();
    let mut plan_us = Vec::new();
    let mut total_us = Vec::new();
    let mut validate_us = Vec::new();
    let mut shape = (0, 0, 0);
    for step in 0..400 {
        let shift = (step as f32) * 3.7;
        scene.update(
            glide,
            DisplayItem::PushTransform(Transform2D::translate(0.0, -shift)),
        );
        let started = Instant::now();
        let (commands, uploads, lowered) =
            lowering.commands(&scene, VIEWPORT, SCALE, ProtoInstant::ZERO);
        let lowered_at = started.elapsed();
        let plan = plan_paint_with(extent, clear, &commands, limits).unwrap();
        let finished = started.elapsed();
        let validated = Instant::now();
        aqua_graph::validate(&plan).unwrap();
        let validate_cost = validated.elapsed();
        lowering.retain_frame(commands);
        drop(uploads);
        if step >= 40 {
            lower_us.push(lowered_at.as_secs_f64() * 1e6);
            plan_us.push((finished - lowered_at).as_secs_f64() * 1e6);
            total_us.push(finished.as_secs_f64() * 1e6);
            validate_us.push(validate_cost.as_secs_f64() * 1e6);
            shape = (lowered.commands, plan.passes.len(), lowered.glyphs);
        }
    }
    println!(
        "perf-lower shape items={} commands={} passes={} glyphs={}",
        scene.len(),
        shape.0,
        shape.1,
        shape.2
    );
    report("lower", lower_us);
    report("plan", plan_us);
    report("total", total_us);
    report("validate", validate_us);
}
