use aqua_core::{ColorProfile, DisplayItem, GpuiBackground, GpuiHsla};
use aqua_runtime::Runtime;
use aqua_text::{Selection, SharedFonts, SharedRuns, StyledSpan};
use aqua_ui::{
    element::ui::Text,
    style::{px, Style},
    tree::{GpuiTextAppearance, NodeKind},
};

#[test]
fn wrapped_backgrounds_preserve_colors_and_exclude_selection() {
    let background = GpuiHsla {
        h: 0.123456,
        s: 0.74,
        l: 0.42,
        a: 0.65,
    };
    let selection = GpuiHsla {
        h: 0.73,
        s: 0.54,
        l: 0.61,
        a: 0.4,
    };
    let appearance = GpuiTextAppearance {
        selection: aqua_core::GpuiBackground::Solid(selection),
        caret: GpuiHsla {
            h: 0.95,
            ..selection
        },
        caret_width: 2.0,
        ime_underline: selection,
        ime_underline_width: 1.0,
    };
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new())
        .with_color_profile(ColorProfile::GpuiWindowsEncoded);
    runtime
        .mount(|_| {
            Text::builder()
                .spans(vec![StyledSpan::plain(
                    "alpha bravo charlie delta echo foxtrot",
                )
                .gpui_background([
                    background.h,
                    background.s,
                    background.l,
                    background.a,
                ])])
                .wrap(true)
                .editable(true)
                .multiline(true)
                .gpui_text_appearance(appearance)
                .style(Style::new().width(px(80.0)).height(px(100.0)))
                .build()
        })
        .unwrap();
    runtime.set_viewport(160.0, 120.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Text(_)))
        .unwrap();
    let before: Vec<_> = runtime
        .scene()
        .ordered()
        .filter_map(|(_, item)| match item {
            DisplayItem::GpuiBackground {
                bounds,
                background: GpuiBackground::Solid(color),
                ..
            } if *color == background => Some(*bounds),
            _ => None,
        })
        .collect();
    assert!(before.len() > 1);
    assert!(runtime.tree_mut().set_focused(Some(node)));
    assert!(runtime
        .tree_mut()
        .set_text_selection(node, Some(Selection::new(2, 14))));
    runtime.build_frame().unwrap();
    let items: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    let selected: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            DisplayItem::GpuiBackground {
                bounds,
                background: GpuiBackground::Solid(color),
                ..
            } if *color == selection => Some(*bounds),
            _ => None,
        })
        .collect();
    assert!(!selected.is_empty());
    for item in &items {
        if let DisplayItem::GpuiBackground {
            bounds,
            background: GpuiBackground::Solid(color),
            ..
        } = item
        {
            if *color == background {
                assert!(selected
                    .iter()
                    .all(|selection| bounds.intersect(*selection).is_empty()));
            }
        }
    }
    let text = items
        .iter()
        .position(|item| matches!(item, DisplayItem::Text { .. }))
        .unwrap();
    assert!(items[..text].iter().any(|item| matches!(item, DisplayItem::GpuiBackground { background: GpuiBackground::Solid(color), .. } if *color == selection)));
}

#[test]
fn source_background_without_encoded_profile_is_rejected() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Text::builder()
                .spans(vec![
                    StyledSpan::plain("color").gpui_background([0.1, 0.2, 0.3, 1.0])
                ])
                .build()
        })
        .unwrap();
    runtime.set_viewport(100.0, 40.0, 1.0);
    assert!(runtime.build_frame().is_err());
}
