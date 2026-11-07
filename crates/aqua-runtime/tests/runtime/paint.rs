use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use aqua_core::{Oklch, Size};
use aqua_protocol::{RenderCommand, RenderInput};
use aqua_runtime::{emit_scene_diff, RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui;
use aqua_ui::paint::FillGroup;
use aqua_ui::style::{px, Style};

fn colors(width: u32, height: u32, frame: u32) -> impl Iterator<Item = ([u8; 3], f32)> {
    (0..height).flat_map(move |y| {
        (0..width).scan([0u8; 3], move |previous, x| {
            let red = (x.wrapping_pow(2).wrapping_add(frame.wrapping_mul(3)) & 255) as u8;
            let green = (y.wrapping_pow(3).wrapping_add(frame.wrapping_mul(5)) & 255) as u8;
            let blue = (x
                .wrapping_mul(y)
                .wrapping_pow(2)
                .wrapping_add(frame.wrapping_mul(7))
                & 255) as u8;
            *previous = [
                ((u16::from(red) + u16::from(previous[0])) / 2) as u8,
                green,
                blue ^ (previous[0] >> 3),
            ];
            Some((*previous, 1.0))
        })
    })
}

#[test]
#[ignore]
fn retained_million_box_cpu_report() {
    let width = 1280u32;
    let height = 800u32;
    let count = (width * height) as usize;
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(width as f32, height as f32, 1.25);
    let mut initial = Vec::with_capacity(count);
    Oklch::extend_srgb8(&mut initial, colors(width, height, 0));
    runtime
        .mount(|_| {
            ui::Box::new()
                .style(
                    Style::new()
                        .column()
                        .width(px(width as f32))
                        .height(px(height as f32)),
                )
                .children(initial.chunks_exact(width as usize).map(|row| {
                    ui::Box::new()
                        .style(Style::new().row().height(px(1.0)).flex_shrink(0.0))
                        .children(row.iter().map(|color| {
                            ui::Box::new()
                                .style(Style::new().width(px(1.0)).height(px(1.0)).flex_shrink(0.0))
                                .fill(*color)
                                .build()
                        }))
                        .build()
                }))
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let nodes = runtime
        .tree()
        .traverse()
        .into_iter()
        .filter(|node| runtime.tree().get(*node).unwrap().children.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(nodes.len(), count);
    let group = FillGroup::new(runtime.tree(), nodes).unwrap();
    let mut side = RenderSide::new();
    side.step(RenderInput {
        commands: vec![
            RenderCommand::Viewport {
                size: Size::new(width as f32, height as f32),
                scale: 1.25,
            },
            RenderCommand::Scene(emit_scene_diff(runtime.scene_mut())),
        ],
        feedback: Vec::new(),
        present_at: aqua_protocol::Instant::ZERO,
    });
    let (read, write) = runtime.scope().signal(0u32);
    let color_time = Rc::new(Cell::new(0.0));
    let color_sample = Rc::clone(&color_time);
    group.bind(
        runtime.scope(),
        &runtime.tree().command_sink(),
        (read,),
        move |(frame,), output| {
            let begin = Instant::now();
            Oklch::extend_srgb8(output, colors(width, height, frame));
            color_sample.set(begin.elapsed().as_secs_f64() * 1000.0);
        },
    );
    let frames = std::env::var("AQUA_STORM_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(3);
    for frame in 1..=frames {
        let begin = Instant::now();
        write.set(frame);
        runtime.build_frame().unwrap();
        let ui_ms = begin.elapsed().as_secs_f64() * 1000.0;
        let emit = Instant::now();
        let diff = emit_scene_diff(runtime.scene_mut());
        let fills = diff.fills.len();
        assert!(diff.upserts.is_empty());
        assert!(diff.order.is_none());
        assert_eq!(fills, count);
        let emit_ms = emit.elapsed().as_secs_f64() * 1000.0;
        let render = Instant::now();
        side.step(RenderInput {
            commands: vec![RenderCommand::Scene(diff)],
            feedback: Vec::new(),
            present_at: aqua_protocol::Instant::ZERO,
        });
        let render_ms = render.elapsed().as_secs_f64() * 1000.0;
        let total_ms = begin.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(side.scene().len(), count);
        assert_eq!(runtime.frame_work().tree_scene_ops_emitted, 0);
        assert_eq!(runtime.frame_work().text_nodes_checked, 0);
        assert_eq!(runtime.frame_work().layouts, 0);
        assert_eq!(group.take_error(), None);
        let color_ms = color_time.get();
        println!("retained-storm-cpu frame={frame} boxes={count} color_ms={color_ms:.3} ui_ms={ui_ms:.3} emit_ms={emit_ms:.3} render_ms={render_ms:.3} total_ms={total_ms:.3}");
    }
    let mut next = Vec::with_capacity(count);
    Oklch::extend_srgb8(&mut next, colors(width, height, frames + 1));
    let begin = Instant::now();
    group.apply(runtime.tree_mut(), &next).unwrap();
    let apply_ms = begin.elapsed().as_secs_f64() * 1000.0;
    let begin = Instant::now();
    runtime.build_frame().unwrap();
    let build_ms = begin.elapsed().as_secs_f64() * 1000.0;
    println!("retained-storm-direct apply_ms={apply_ms:.3} build_ms={build_ms:.3}");
}
