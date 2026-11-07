use std::hint::black_box;
use std::time::Instant;

use aqua_core::{DisplayItem, Oklch, Size};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::{lower_specs, ui, Element};
use aqua_ui::style::{px, Axis, Paint, Style};
use aqua_ui::{NaiveMeasurer, UiTree};

fn colors(width: u32, height: u32, frame: u32) -> Vec<[u8; 3]> {
    (0..height)
        .flat_map(|y| {
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
                Some(*previous)
            })
        })
        .collect()
}

fn converted(colors: &[[u8; 3]]) -> Vec<Oklch> {
    colors
        .iter()
        .map(|&[r, g, b]| Oklch::from_srgb8(r, g, b, 1.0))
        .collect()
}

fn grid(width: u32, colors: &[Oklch]) -> Element {
    ui::Box::new()
        .style(
            Style::new()
                .axis(Axis::Column)
                .width(px(width as f32))
                .height(px((colors.len() / width as usize) as f32)),
        )
        .children(colors.chunks_exact(width as usize).map(|row| {
            ui::Box::new()
                .style(
                    Style::new()
                        .axis(Axis::Row)
                        .width(px(width as f32))
                        .height(px(1.0))
                        .flex_shrink(0.0),
                )
                .children(row.iter().map(|color| {
                    ui::Box::new()
                        .style(
                            Style::new()
                                .width(px(1.0))
                                .height(px(1.0))
                                .flex_shrink(0.0)
                                .background(*color),
                        )
                        .build()
                }))
                .build()
        }))
        .build()
}

#[test]
fn color_refresh_preserves_identity_layout_and_updates_every_cell() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let initial = converted(&colors(16, 10, 0));
    view.set_content(&mut tree, vec![grid(16, &initial)])
        .unwrap();
    tree.drain_commands();
    tree.compute_layout(Size::new(16.0, 10.0), &NaiveMeasurer)
        .unwrap();
    let ids = tree.traverse();
    let rects: Vec<_> = ids.iter().map(|id| tree.rect(*id)).collect();
    let version = tree.layout_version();
    let next = converted(&colors(16, 10, 1));
    view.set_content(&mut tree, vec![grid(16, &next)]).unwrap();
    tree.drain_commands();
    tree.compute_layout(Size::new(16.0, 10.0), &NaiveMeasurer)
        .unwrap();
    assert_eq!(tree.traverse(), ids);
    assert_eq!(
        ids.iter().map(|id| tree.rect(*id)).collect::<Vec<_>>(),
        rects
    );
    assert_eq!(tree.layout_version(), version);
    let fills: Vec<_> = tree
        .emit_scene()
        .into_iter()
        .filter_map(|(_, item)| match item {
            DisplayItem::Rect { fill, .. } | DisplayItem::Quad { fill, .. } => Some(fill),
            _ => None,
        })
        .collect();
    assert_eq!(fills, next);
    assert_ne!(initial, next);
}

#[test]
fn invalid_later_sibling_does_not_commit_earlier_paint_changes() {
    use aqua_core::Transform2D;
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let content = |first, second| {
        vec![ui::Box::new()
            .child(ui::Box::new().fill(first).build())
            .child(second)
            .build()]
    };
    let root = view
        .set_content(
            &mut tree,
            content(Oklch::BLACK, ui::Box::new().fill(Oklch::BLACK).build()),
        )
        .unwrap()[0];
    tree.drain_commands();
    let first = tree.get(root).unwrap().children[0];
    let invalid = ui::Box::new()
        .style(Style::new().transform(Transform2D {
            a: 0.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: 0.0,
            ty: 0.0,
        }))
        .build();
    assert!(view
        .set_content(&mut tree, content(Oklch::WHITE, invalid))
        .is_err());
    assert_eq!(tree.paint(first).unwrap().fill, Some(Oklch::BLACK));
}

#[test]
fn reordered_boxes_keep_their_keyed_identity() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let content = |keys: [u32; 2]| {
        vec![ui::Box::new()
            .children(keys.map(|key| ui::Box::new().fill(Oklch::BLACK).build().with_key(key)))
            .build()]
    };
    let root = view.set_content(&mut tree, content([1, 2])).unwrap()[0];
    tree.drain_commands();
    let original = tree.get(root).unwrap().children.clone();
    view.set_content(&mut tree, content([2, 1])).unwrap();
    tree.drain_commands();
    assert_eq!(
        tree.get(root).unwrap().children,
        vec![original[1], original[0]]
    );
}

fn measure<T>(label: &str, count: usize, run: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = black_box(run());
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "storm-cpu phase={label} boxes={count} ms={:.3} ns_per_box={:.1}",
        elapsed * 1000.0,
        elapsed * 1e9 / count as f64
    );
    result
}

#[test]
#[ignore]
fn pixel_storm_phase_report() {
    println!(
        "storm-cpu bytes element={} spec={} record={} paint={} display_item={}",
        std::mem::size_of::<Element>(),
        std::mem::size_of::<aqua_ui::NodeSpec>(),
        std::mem::size_of::<aqua_ui::NodeRecord>(),
        std::mem::size_of::<Paint>(),
        std::mem::size_of::<DisplayItem>()
    );
    let sizes = std::env::var("AQUA_STORM_SIZES").unwrap_or_else(|_| "320x200,640x400".into());
    for size in sizes.split(',') {
        let (width, height) = size.split_once('x').unwrap();
        let width: u32 = width.parse().unwrap();
        let height: u32 = height.parse().unwrap();
        let count = (width * height) as usize;
        let runtime = Runtime::new();
        let mut tree = UiTree::new();
        let mut view = View::new(runtime.create_scope(), tree.command_sink());
        let initial = converted(&colors(width, height, 0));
        view.set_content(&mut tree, vec![grid(width, &initial)])
            .unwrap();
        tree.drain_commands();
        tree.compute_layout(Size::new(width as f32, height as f32), &NaiveMeasurer)
            .unwrap();
        black_box(tree.emit_scene());
        for frame in 1..=3 {
            println!("storm-cpu sample={frame} grid={size}");
            let colors = measure("integer_colors", count, || colors(width, height, frame));
            let colors = measure("srgb_to_oklch", count, || converted(&colors));
            let gpu_colors = measure("oklch_to_gpu", count, || {
                colors
                    .iter()
                    .copied()
                    .map(Oklch::to_gpu)
                    .collect::<Vec<_>>()
            });
            drop(gpu_colors);
            let element = measure("element_build", count, || grid(width, &colors));
            let specs = measure("lower_specs_diagnostic", count, || {
                lower_specs(&element).unwrap()
            });
            drop(specs);
            measure("set_content", count, || {
                view.set_content(&mut tree, vec![element]).unwrap()
            });
            measure("drain_commands", count, || tree.drain_commands());
            measure("layout", count, || {
                tree.compute_layout(Size::new(width as f32, height as f32), &NaiveMeasurer)
                    .unwrap()
            });
            let items = measure("emit_scene", count, || tree.emit_scene());
            assert_eq!(items.len(), count);
            drop(items);
            let ids: Vec<_> = tree
                .traverse()
                .into_iter()
                .filter(|id| tree.get(*id).unwrap().children.is_empty())
                .collect();
            measure("direct_paint_write", count, || {
                for (id, color) in ids.iter().zip(&colors) {
                    tree.set_paint(
                        *id,
                        Paint {
                            fill: Some(Oklch {
                                alpha: 0.9,
                                ..*color
                            }),
                            ..Paint::default()
                        },
                    );
                }
            });
            let items = measure("emit_after_direct_paint", count, || tree.emit_scene());
            assert_eq!(items.len(), count);
            drop(items);
        }
    }
}

#[test]
#[ignore]
fn pixel_storm_color_report() {
    for slots in [256usize, 4096, 65536] {
        let mut entries = vec![None; slots];
        let mut gpu_entries = vec![None; slots];
        for frame in 1..=3 {
            let input = colors(1280, 800, frame);
            let reference = converted(&input);
            let mut misses = 0usize;
            let output = measure("memoized_srgb_to_oklch", input.len(), || {
                input
                    .iter()
                    .map(|&[r, g, b]| {
                        let key = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
                        let slot = (key.wrapping_mul(0x9e3779b9) >> (32 - slots.ilog2())) as usize;
                        match entries[slot] {
                            Some((existing, color)) if existing == key => color,
                            _ => {
                                misses += 1;
                                let color = Oklch::from_srgb8(r, g, b, 1.0);
                                entries[slot] = Some((key, color));
                                color
                            }
                        }
                    })
                    .collect::<Vec<_>>()
            });
            assert_eq!(output, reference);
            println!(
                "storm-cpu memo_slots={slots} sample={frame} misses={misses} values={}",
                input.len()
            );
            let gpu_reference: Vec<_> = reference.iter().copied().map(Oklch::to_gpu).collect();
            let mut gpu_misses = 0usize;
            let gpu_output = measure("memoized_oklch_to_gpu", input.len(), || {
                reference
                    .iter()
                    .copied()
                    .map(|color| {
                        let key = [
                            color.l.to_bits(),
                            color.c.to_bits(),
                            color.h.to_bits(),
                            color.alpha.to_bits(),
                        ];
                        let mixed = key.iter().fold(0x9e3779b9u32, |hash, word| {
                            (hash ^ word).wrapping_mul(0x85ebca6b).rotate_left(13)
                        });
                        let slot = mixed as usize & (slots - 1);
                        match gpu_entries[slot] {
                            Some((existing, packed)) if existing == key => packed,
                            _ => {
                                gpu_misses += 1;
                                let packed = color.to_gpu();
                                gpu_entries[slot] = Some((key, packed));
                                packed
                            }
                        }
                    })
                    .collect::<Vec<_>>()
            });
            assert_eq!(gpu_output, gpu_reference);
            println!(
                "storm-cpu gpu_memo_slots={slots} sample={frame} misses={gpu_misses} values={}",
                input.len()
            );
        }
    }
}
