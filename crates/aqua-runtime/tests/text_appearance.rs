use aqua_core::{DisplayItem, Oklch};
use aqua_runtime::Runtime;
use aqua_text::{Selection, SharedFonts, SharedRuns};
use aqua_ui::{
    element::ui::Text,
    style::{px, Style},
    tree::{NodeKind, TextAppearance, TextBacking},
};

#[test]
fn selection_caret_and_composition_use_text_appearance_in_logical_pixels() {
    let appearance = TextAppearance {
        selection: Oklch::from_hex("#ff0000").unwrap(),
        caret: Oklch::from_hex("#00ff00").unwrap(),
        caret_width: 2.0,
        ime_underline: Oklch::from_hex("#0000ff").unwrap(),
        ime_underline_width: 1.0,
    };
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Text::builder()
                .arg("hello")
                .editable(true)
                .text_appearance(appearance)
                .style(Style::new().width(px(200.0)).height(px(40.0)))
                .build()
        })
        .unwrap();
    runtime.set_viewport(400.0, 100.0, 2.0);
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Text(_)))
        .unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime
        .tree_mut()
        .set_text_selection(node, Some(Selection::new(0, 3)));
    runtime.build_frame().unwrap();
    let items: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    assert!(items.iter().any(
        |item| matches!(item, DisplayItem::Rect { fill, .. } if *fill == appearance.selection)
    ));
    assert!(items.iter().any(|item| matches!(item, DisplayItem::Rect { fill, bounds, .. } if *fill == appearance.caret && (bounds.w - 2.0).abs() < 0.001)));
    runtime.tree_mut().edit_text(node, |text| {
        if let TextBacking::Editable(editor) = &mut text.backing {
            editor.set_composition("ab", 0..2);
        }
    });
    runtime.build_frame().unwrap();
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::Rect { fill, bounds, .. } if *fill == appearance.ime_underline && (bounds.h - 1.0).abs() < 0.001)));
}

#[test]
fn source_text_appearance_preserves_float_colors_and_metrics_in_scene() {
    use aqua_core::{GpuiBackground, GpuiHsla};
    use aqua_ui::tree::GpuiTextAppearance;
    let color = |h, a| GpuiHsla {
        h,
        s: 0.731,
        l: 0.421,
        a,
    };
    let appearance = GpuiTextAppearance {
        selection: color(0.123, 0.37),
        caret: color(0.456, 1.0),
        caret_width: 2.0,
        ime_underline: color(0.789, 0.81),
        ime_underline_width: 1.0,
    };
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Text::builder()
                .arg("hello")
                .editable(true)
                .gpui_text_appearance(appearance)
                .style(Style::new().width(px(200.0)).height(px(40.0)))
                .build()
        })
        .unwrap();
    runtime.set_viewport(400.0, 100.0, 2.0);
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Text(_)))
        .unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime
        .tree_mut()
        .set_text_selection(node, Some(Selection::new(0, 3)));
    runtime.build_frame().unwrap();
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::GpuiBackground { background: GpuiBackground::Solid(color), .. } if *color == appearance.selection)));
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::GpuiBackground { bounds, background: GpuiBackground::Solid(color), .. } if *color == appearance.caret && (bounds.w - 2.0).abs() < 0.001)));
    runtime.tree_mut().edit_text(node, |text| {
        if let TextBacking::Editable(editor) = &mut text.backing {
            editor.set_composition("ab", 0..2);
        }
    });
    runtime.build_frame().unwrap();
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::GpuiBackground { bounds, background: GpuiBackground::Solid(color), .. } if *color == appearance.ime_underline && (bounds.h - 1.0).abs() < 0.001)));
}
