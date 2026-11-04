use aqua_core::{DisplayItem, NodeId};
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::style::{Axis, Style, px};
use aqua_ui::tree::NodeKind;

fn transcript() -> (Runtime, Vec<NodeId>) {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(800.0, 600.0, 1.0);
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column))
                .children((0..1000).map(|index| {
                    UiText::builder()
                        .arg(format!(
                            "Message {index}: a streaming conversation with retained text."
                        ))
                        .style(Style::new().width(px(720.0)).height(px(24.0)))
                        .build()
                }))
                .child(
                    UiText::builder()
                        .arg("draft")
                        .editable(true)
                        .style(Style::new().width(px(720.0)).height(px(24.0)))
                        .build(),
                )
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let nodes = runtime
        .tree()
        .traverse()
        .into_iter()
        .filter(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Text(_)))
        .collect();
    (runtime, nodes)
}

fn items(runtime: &Runtime) -> Vec<DisplayItem> {
    runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect()
}

#[test]
fn idle_transcript_frames_reuse_the_scene_without_inspecting_text() {
    let (mut runtime, _) = transcript();
    let expected = items(&runtime);
    for frame in 1..5 {
        runtime.advance_clock(frame as f64 / 60.0);
        runtime.build_frame().unwrap();
        let work = runtime.frame_work();
        assert!(runtime.frame_stats().layout_skipped);
        assert!(work.tree_scene_reused);
        assert_eq!(work.text_nodes_checked, 0);
        assert_eq!(work.text_source_bytes, 0);
        assert_eq!(work.text_runs_shaped, 0);
        assert_eq!(work.tree_scene_ops_emitted, 0);
        assert_eq!(work.scene_items_reconciled, 0);
        assert_eq!(items(&runtime), expected);
    }
}

#[test]
fn streaming_a_message_shapes_only_its_changed_source() {
    let (mut runtime, nodes) = transcript();
    let mut message = String::from("Streaming");
    for _ in 0..5 {
        message.push_str(" another token");
        runtime.tree_mut().set_text(nodes[999], message.clone());
        runtime.build_frame().unwrap();
        let work = runtime.frame_work();
        assert_eq!(work.text_nodes_checked, 1001);
        assert_eq!(work.text_source_bytes, message.len());
        assert_eq!(work.text_runs_shaped, 1);
        assert!(!work.tree_scene_reused);
        assert!(work.tree_scene_ops_emitted >= 1001);
    }
}

#[test]
fn moving_the_composer_caret_does_not_reshape_the_transcript() {
    let (mut runtime, nodes) = transcript();
    let composer = nodes[1000];
    runtime.tree_mut().set_focused(Some(composer));
    runtime.build_frame().unwrap();
    runtime
        .tree_mut()
        .set_text_selection(composer, Some(aqua_text::Selection::caret_at(2)));
    runtime.build_frame().unwrap();
    let work = runtime.frame_work();
    assert_eq!(work.text_source_bytes, 0);
    assert!(work.tree_scene_reused);
    assert!(runtime.frame_stats().layout_skipped);
    assert_eq!(work.text_runs_shaped, 0);
    runtime.build_frame().unwrap();
    assert!(runtime.frame_work().tree_scene_reused);
    assert_eq!(runtime.frame_work().text_nodes_checked, 0);
    assert!(runtime.frame_work().scene_items_reconciled < 10);
}

#[test]
fn viewport_and_scale_changes_invalidate_cached_geometry_and_runs() {
    let (mut runtime, _) = transcript();
    runtime.set_viewport(900.0, 600.0, 1.0);
    runtime.build_frame().unwrap();
    assert!(!runtime.frame_work().tree_scene_reused);
    runtime.set_viewport(900.0, 600.0, 2.0);
    runtime.build_frame().unwrap();
    assert!(!runtime.frame_work().tree_scene_reused);
    assert_eq!(runtime.frame_work().text_runs_shaped, 1001);
}

#[test]
fn changing_selection_preserves_base_scene_ids_and_paints_behind_glyphs() {
    let (mut runtime, nodes) = transcript();
    let composer = nodes[1000];
    runtime.tree_mut().set_focused(Some(composer));
    runtime.build_frame().unwrap();
    let text_ids: Vec<_> = runtime
        .scene()
        .ordered()
        .filter_map(|(id, item)| matches!(item, DisplayItem::Text { .. }).then_some(id))
        .collect();
    runtime.scene_mut().drain_changes();
    runtime
        .tree_mut()
        .set_text_selection(composer, Some(aqua_text::Selection::new(0, 3)));
    runtime.build_frame().unwrap();
    assert!(runtime.frame_work().tree_scene_reused);
    assert_eq!(runtime.frame_work().text_source_bytes, 0);
    let updated_text_ids: Vec<_> = runtime
        .scene()
        .ordered()
        .filter_map(|(id, item)| matches!(item, DisplayItem::Text { .. }).then_some(id))
        .collect();
    assert_eq!(text_ids, updated_text_ids);
    assert!(runtime.scene().pending_change_count() < 10);
    let scene = items(&runtime);
    let selection_index = scene
        .iter()
        .position(|item| matches!(item, DisplayItem::Rect { fill, .. } if fill.alpha == 0.35))
        .unwrap();
    let composer_index = scene
        .iter()
        .rposition(|item| matches!(item, DisplayItem::Text { .. }))
        .unwrap();
    assert!(selection_index < composer_index);
}

#[test]
fn loading_a_font_invalidates_idle_text_layout_and_run_revisions() {
    let fonts = SharedFonts::new();
    let bytes = {
        let mut manager = fonts.lock();
        let face = manager.first_face().unwrap();
        manager.export_face_data(face).unwrap()
    };
    let mut runtime = Runtime::new(fonts.clone(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiText::builder()
                .arg("Font generation changes must reshape retained text")
                .multiline(true)
                .build()
        })
        .unwrap();
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let revision = runtime
        .scene()
        .ordered()
        .find_map(|(_, item)| match item {
            DisplayItem::Text { run, .. } => Some(run.revision),
            _ => None,
        })
        .unwrap();
    runtime.build_frame().unwrap();
    assert!(runtime.frame_work().tree_scene_reused);
    fonts.lock().load_font_bytes(bytes);
    runtime.build_frame().unwrap();
    assert!(!runtime.frame_stats().layout_skipped);
    assert!(!runtime.frame_work().tree_scene_reused);
    assert_eq!(runtime.frame_work().text_runs_shaped, 1);
    let updated = runtime
        .scene()
        .ordered()
        .find_map(|(_, item)| match item {
            DisplayItem::Text { run, .. } => Some(run.revision),
            _ => None,
        })
        .unwrap();
    assert!(updated > revision);
}

#[test]
fn animation_deadlines_and_reduced_motion_preserve_unchanged_tree_cache() {
    use aqua_ui::motion::{Easing, MotionKind, MotionValue};

    let (mut runtime, _) = transcript();
    let motion = runtime.host_mut().motion_mut();
    let handle = motion.spawn(
        None,
        MotionValue::Scalar(0.0),
        MotionKind::Tween {
            duration_secs: 1.0,
            easing: Easing::Linear,
        },
        0.0,
    );
    motion
        .set_target(handle, MotionValue::Scalar(1.0), 0.0)
        .unwrap();
    runtime.advance_clock(0.1);
    assert!(runtime.build_frame().unwrap().is_some());
    assert!(runtime.frame_work().tree_scene_reused);
    assert_eq!(runtime.frame_work().text_nodes_checked, 0);
    runtime.apply_startup_motion_policy(true);
    runtime.advance_clock(0.2);
    assert!(runtime.build_frame().unwrap().is_none());
    assert!(runtime.frame_work().tree_scene_reused);
    assert_eq!(runtime.frame_work().text_runs_shaped, 0);
}

#[test]
fn control_word_and_document_selection_reuse_the_entire_transcript_scene() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_text::Selection;
    use aqua_ui::tree::TextBacking;

    let (mut runtime, nodes) = transcript();
    let composer = nodes[1000];
    runtime.tree_mut().set_focused(Some(composer));
    runtime.build_frame().unwrap();
    let control_shift = Modifiers::CONTROL.union(Modifiers::SHIFT);
    let steps = [
        (
            Key::Named(NamedKey::Home),
            Modifiers::CONTROL,
            Selection::caret_at(0),
        ),
        (
            Key::Named(NamedKey::Right),
            Modifiers::CONTROL,
            Selection::caret_at(5),
        ),
        (
            Key::Named(NamedKey::Home),
            control_shift,
            Selection::new(5, 0),
        ),
        (Key::Char('a'), Modifiers::CONTROL, Selection::new(0, 5)),
        (
            Key::Named(NamedKey::End),
            Modifiers::CONTROL,
            Selection::caret_at(5),
        ),
        (
            Key::Named(NamedKey::Left),
            Modifiers::CONTROL,
            Selection::caret_at(0),
        ),
    ];
    for (key, modifiers, expected) in steps {
        assert!(runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
            key,
            modifiers,
            repeat: false
        })));
        let NodeKind::Text(text) = &runtime.tree().get(composer).unwrap().kind else {
            panic!()
        };
        let TextBacking::Editable(editor) = &text.backing else {
            panic!()
        };
        assert_eq!(editor.selection(), expected);
        runtime.build_frame().unwrap();
        let work = runtime.frame_work();
        assert!(runtime.frame_stats().layout_skipped);
        assert!(work.tree_scene_reused);
        assert_eq!(work.text_nodes_checked, 0);
        assert_eq!(work.text_source_bytes, 0);
        assert_eq!(work.text_runs_shaped, 0);
        assert_eq!(work.tree_scene_ops_emitted, 0);
    }
}

#[test]
fn zz_probe() {
    let (mut runtime, nodes) = transcript();
    runtime.advance_clock(1.0);
    runtime.build_frame().unwrap();
    eprintln!("idle {:?}", runtime.frame_work());
    runtime
        .tree_mut()
        .set_text(nodes[999], "changed".to_string());
    runtime.build_frame().unwrap();
    eprintln!("stream {:?}", runtime.frame_work());
    runtime.build_frame().unwrap();
    eprintln!("after {:?}", runtime.frame_work());
}
