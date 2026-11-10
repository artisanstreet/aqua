use aqua_core::{DisplayItem, Oklch, Rect, Size};
use aqua_motion::{Binding, Easing, MotionCommand, MotionKind, MotionValue, Property, TrackId};
use aqua_protocol::{Instant, RenderCommand, RenderInput};
use aqua_runtime::{emit_scene_diff, RenderSide};
use aqua_scene::Scene;

fn filled(color: Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        },
        radius: 0.0,
        fill: color,
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    }
}

fn step(side: &mut RenderSide, commands: Vec<RenderCommand>, seconds: f64) {
    side.step(RenderInput {
        commands,
        feedback: Vec::new(),
        present_at: Instant::from_secs_f64(seconds),
    });
}

#[test]
fn emitted_fill_deltas_preserve_render_identity_and_stale_handles_fail_closed() {
    let mut source = Scene::new();
    let node = source.insert(filled(Oklch::BLACK));
    let mut side = RenderSide::new();
    step(
        &mut side,
        vec![
            RenderCommand::Viewport {
                size: Size::new(20.0, 20.0),
                scale: 1.0,
            },
            RenderCommand::Scene(emit_scene_diff(&mut source)),
        ],
        0.0,
    );
    let local = side.scene().ordered().next().unwrap().0;
    source.update_fill(node, Oklch::WHITE);
    let diff = emit_scene_diff(&mut source);
    assert!(diff.upserts.is_empty());
    assert!(diff.order.is_none());
    assert_eq!(diff.fills.len(), 1);
    let stale = diff.clone();
    step(&mut side, vec![RenderCommand::Scene(diff)], 0.0);
    assert_eq!(side.presented(node), Some(&filled(Oklch::WHITE)));
    assert_eq!(side.scene().ordered().next().unwrap().0, local);
    source.remove(node);
    let replacement = source.insert(filled(Oklch::BLACK));
    step(
        &mut side,
        vec![
            RenderCommand::Scene(emit_scene_diff(&mut source)),
            RenderCommand::Scene(stale),
        ],
        0.0,
    );
    assert_eq!(side.presented(node), None);
    assert_eq!(side.presented(replacement), Some(&filled(Oklch::BLACK)));
}

#[test]
fn fill_deltas_update_the_base_under_an_active_motion_override() {
    for native in [false, true] {
        let mut source = Scene::new();
        let mut initial = filled(Oklch::BLACK);
        if native {
            initial.set_srgba(aqua_core::Srgba8::new(0, 0, 0, 255));
        }
        let node = source.insert(initial);
        let mut side = RenderSide::new();
        let track = TrackId::from_raw(aqua_core::RawId {
            index: 0,
            generation: 0,
        });
        let kind = MotionKind::Tween {
            duration_secs: 1.0,
            easing: Easing::Linear,
        };
        step(
            &mut side,
            vec![
                RenderCommand::Scene(emit_scene_diff(&mut source)),
                RenderCommand::Motion(MotionCommand::Spawn {
                    track,
                    binding: Some(Binding {
                        node,
                        property: Property::Fill,
                    }),
                    initial: MotionValue::Color(Oklch::BLACK),
                    kind,
                    at: 0.0,
                }),
                RenderCommand::Motion(MotionCommand::SetTarget {
                    track,
                    generation: 0,
                    to: MotionValue::Color(Oklch::WHITE),
                    kind,
                    at: 0.0,
                }),
            ],
            0.5,
        );
        let before = side.presented(node).cloned();
        let color = aqua_core::Srgba8::new(13, 123, 231, 255);
        let updated = color.to_oklch();
        let mut expected = filled(updated);
        if native {
            expected.set_srgba(color);
            let mut batch = aqua_core::SrgbaFillBatch::default();
            batch.push(aqua_core::SrgbaFillPatch { node, fill: color });
            source.apply_srgba_batch(batch);
        } else {
            source.update_fill(node, updated);
        }
        step(
            &mut side,
            vec![RenderCommand::Scene(emit_scene_diff(&mut source))],
            0.5,
        );
        assert_eq!(side.presented(node), before.as_ref());
        step(
            &mut side,
            vec![RenderCommand::Motion(MotionCommand::Unbind { track })],
            0.5,
        );
        assert_eq!(side.presented(node), Some(&expected));
    }
}

#[test]
fn bulk_ui_fills_reuse_scene_and_text_then_fall_back_for_geometry_changes() {
    use aqua_runtime::Runtime;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::ui;
    use aqua_ui::paint::FillGroup;
    use aqua_ui::style::{px, Style};
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(100.0, 100.0, 1.0);
    runtime
        .mount(|_| {
            ui::Box::new()
                .style(Style::new().width(px(100.0)).height(px(100.0)).clip(true))
                .child(
                    ui::Box::new()
                        .style(Style::new().width(px(10.0)).height(px(10.0)))
                        .fill(Oklch::BLACK)
                        .build()
                        .with_key("paint"),
                )
                .child(ui::Text::new("kept").editable(true).build())
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let root = runtime.tree().root().unwrap();
    let node = runtime.tree().get(root).unwrap().children[0];
    let group = FillGroup::new(runtime.tree(), vec![node]).unwrap();
    let before: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(node, item)| (node, item.clone()))
        .collect();
    let geometry = runtime.scene().geometry_revision();
    group.apply(runtime.tree_mut(), &[Oklch::WHITE]).unwrap();
    runtime.build_frame().unwrap();
    let work = runtime.frame_work();
    assert_eq!(work.tree_scene_ops_emitted, 0);
    assert_eq!(work.text_nodes_checked, 0);
    assert_eq!(work.layouts, 0);
    assert_eq!(work.scene_items_reconciled, 1);
    assert_eq!(runtime.scene().geometry_revision(), geometry);
    let after: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(node, item)| (node, item.clone()))
        .collect();
    assert_eq!(
        before.iter().map(|entry| entry.0).collect::<Vec<_>>(),
        after.iter().map(|entry| entry.0).collect::<Vec<_>>()
    );
    let mut expected = before;
    let item = expected
        .iter_mut()
        .find(|(_, item)| matches!(item, DisplayItem::Rect { .. }))
        .unwrap();
    if let DisplayItem::Rect { fill, .. } = &mut item.1 {
        *fill = Oklch::WHITE;
    }
    assert_eq!(after, expected);
    let text = runtime.tree().get(root).unwrap().children[1];
    assert!(runtime.tree_mut().set_focused(Some(text)));
    runtime.build_frame().unwrap();
    group.apply(runtime.tree_mut(), &[Oklch::BLACK]).unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(runtime.frame_work().tree_scene_ops_emitted, 0);
    assert!(runtime
        .tree_mut()
        .set_text_selection(text, Some(aqua_text::Selection::new(0, 1))));
    runtime.build_frame().unwrap();
    assert_eq!(runtime.frame_work().tree_scene_ops_emitted, 0);
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::Rect { bounds, fill, .. } if bounds.w == 10.0 && *fill == Oklch::BLACK)));
    let mut style = runtime.tree().get(node).unwrap().style.clone();
    style.width = aqua_ui::style::Length::Px(20.0);
    runtime.tree_mut().set_style(node, style);
    group.apply(runtime.tree_mut(), &[Oklch::BLACK]).unwrap();
    runtime.build_frame().unwrap();
    assert!(runtime.frame_work().tree_scene_ops_emitted > 0);
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::Rect { bounds, fill, .. } if bounds.w == 20.0 && *fill == Oklch::BLACK)));
}

#[test]
fn native_ui_batches_reach_render_nodes_without_rebuilding_geometry() {
    use aqua_core::Srgba8;
    use aqua_runtime::Runtime;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::ui;
    use aqua_ui::paint::FillGroup;
    use aqua_ui::style::{px, Style};
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(100.0, 100.0, 1.25);
    runtime
        .mount(|_| {
            ui::Box::new()
                .style(Style::new().width(px(10.0)).height(px(10.0)))
                .fill(Oklch::BLACK)
                .build()
        })
        .unwrap();
    let node = runtime.tree().root().unwrap();
    let group = FillGroup::new(runtime.tree(), vec![node]).unwrap();
    let red = Srgba8::new(255, 0, 0, 255);
    let green = Srgba8::new(0, 255, 0, 102);
    group.apply_srgba(runtime.tree_mut(), &[red]).unwrap();
    runtime.build_frame().unwrap();
    let initial = emit_scene_diff(runtime.scene_mut());
    let remote = initial.upserts[0].0;
    let mut side = RenderSide::new();
    step(&mut side, vec![RenderCommand::Scene(initial)], 0.0);
    let geometry = runtime.scene().geometry_revision();
    group.apply_srgba(runtime.tree_mut(), &[green]).unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(runtime.frame_work().tree_scene_ops_emitted, 0);
    assert_eq!(runtime.frame_work().layouts, 0);
    assert_eq!(runtime.scene().geometry_revision(), geometry);
    let changed = emit_scene_diff(runtime.scene_mut());
    assert!(changed.upserts.is_empty());
    assert_eq!(
        changed
            .srgba_fills
            .iter()
            .map(|batch| batch.len())
            .sum::<usize>(),
        1
    );
    step(&mut side, vec![RenderCommand::Scene(changed)], 0.1);
    assert!(
        matches!(side.presented(remote), Some(DisplayItem::SrgbaRect { fill, .. }) if *fill == green)
    );
    group.apply(runtime.tree_mut(), &[Oklch::WHITE]).unwrap();
    runtime.build_frame().unwrap();
    assert!(runtime.frame_work().tree_scene_ops_emitted > 0);
    let changed = emit_scene_diff(runtime.scene_mut());
    step(&mut side, vec![RenderCommand::Scene(changed)], 0.2);
    assert!(
        matches!(side.presented(remote), Some(DisplayItem::Rect { fill, .. }) if *fill == Oklch::WHITE)
    );
}
