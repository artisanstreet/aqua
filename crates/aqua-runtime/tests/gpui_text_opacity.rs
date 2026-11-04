use aqua_core::{scene::GpuiTextOpacityRange, ColorProfile, DisplayItem};
use aqua_runtime::Runtime;
use aqua_text::{Selection, SharedFonts, SharedRuns};
use aqua_ui::{element::ui::Text, tree::NodeKind};
use std::sync::Arc;

#[test]
fn opacity_updates_preserve_shaping_and_selection() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new())
        .with_color_profile(ColorProfile::GpuiWindowsEncoded);
    runtime
        .mount(|_| Text::new("é alpha beta").editable(true).build())
        .unwrap();
    runtime.set_viewport(200.0, 60.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Text(_)))
        .unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime
        .tree_mut()
        .set_text_selection(node, Some(Selection::new(0, 2)));
    runtime.build_frame().unwrap();
    let before = runtime
        .scene()
        .ordered()
        .find_map(|(_, item)| match item {
            DisplayItem::Text { run, .. } => Some(run.clone()),
            _ => None,
        })
        .unwrap();
    let ranges: Arc<[GpuiTextOpacityRange]> = vec![
        GpuiTextOpacityRange {
            bytes: 0..2,
            opacity: 0.25,
        },
        GpuiTextOpacityRange {
            bytes: 3..8,
            opacity: 0.0,
        },
    ]
    .into();
    assert!(runtime
        .tree_mut()
        .set_gpui_text_opacity(node, ranges.clone()));
    runtime.build_frame().unwrap();
    assert_eq!(runtime.frame_work().text_runs_shaped, 0);
    let items: Vec<_> = runtime.scene().ordered().map(|(_, item)| item).collect();
    let push = items.iter().position(|item| matches!(item, DisplayItem::PushGpuiTextOpacity(found) if found.as_ref() == ranges.as_ref())).unwrap();
    let text = items
        .iter()
        .position(|item| matches!(item, DisplayItem::Text { .. }))
        .unwrap();
    let pop = items
        .iter()
        .position(|item| matches!(item, DisplayItem::PopGpuiTextOpacity))
        .unwrap();
    assert!(push < text && text < pop);
    let after = match items[text] {
        DisplayItem::Text { run, .. } => run,
        _ => unreachable!(),
    };
    assert_eq!(before.id, after.id);
    assert_eq!(before.revision, after.revision);
    let NodeKind::Text(text) = &runtime.tree().get(node).unwrap().kind else {
        panic!("text node")
    };
    let aqua_ui::tree::TextBacking::Editable(editor) = &text.backing else {
        panic!("editable text")
    };
    assert_eq!(editor.selection().range(), 0..2);
    assert_eq!(text.text(), "é alpha beta");
    let invalid: Arc<[GpuiTextOpacityRange]> = vec![GpuiTextOpacityRange {
        bytes: 1..2,
        opacity: 0.2,
    }]
    .into();
    assert!(!runtime.tree_mut().set_gpui_text_opacity(node, invalid));
    runtime.build_frame().unwrap();
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::PushGpuiTextOpacity(found) if found.as_ref() == ranges.as_ref())));
    let retry: Arc<[GpuiTextOpacityRange]> = vec![GpuiTextOpacityRange {
        bytes: 0..2,
        opacity: 0.75,
    }]
    .into();
    assert!(runtime
        .tree_mut()
        .set_gpui_text_opacity(node, retry.clone()));
    runtime.build_frame().unwrap();
    assert_eq!(runtime.frame_work().text_runs_shaped, 0);
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::PushGpuiTextOpacity(found) if found.as_ref() == retry.as_ref())));
}
