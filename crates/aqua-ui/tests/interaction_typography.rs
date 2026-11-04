use aqua_core::{GpuiHsla, Oklch};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::ui::{Box, Text};
use aqua_ui::style::Style;
use aqua_ui::tree::interaction::{InputModality, InteractionCommand, InteractionEvent};
use aqua_ui::tree::{NodeKind, UiTree};

#[test]
fn hover_color_inherits_through_descendants_and_restores_explicit_colors() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let element = Box::new()
        .child(Text::new("inherited").build())
        .child(Text::new("explicit").color(Oklch::WHITE).build())
        .build()
        .style(Style::new().text_color(Oklch::BLACK))
        .hover_style(Style::new().text_color(Oklch::TRANSPARENT));
    let root = view.set_content(&mut tree, vec![element]).unwrap()[0];
    tree.drain_commands();
    let children = tree.get(root).unwrap().children.clone();
    let colors = |tree: &UiTree| {
        children
            .iter()
            .map(|node| match &tree.get(*node).unwrap().kind {
                NodeKind::Text(text) => text.color,
                _ => panic!("expected text"),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(colors(&tree), vec![Oklch::BLACK, Oklch::WHITE]);
    tree.queue_interaction_event(root, InteractionEvent::Hover(true));
    tree.drain_commands();
    assert_eq!(colors(&tree), vec![Oklch::TRANSPARENT, Oklch::WHITE]);
    tree.command_sink()
        .interaction(InteractionCommand::Modality(InputModality::Keyboard));
    tree.drain_commands();
    assert_eq!(colors(&tree), vec![Oklch::BLACK, Oklch::WHITE]);
    tree.command_sink()
        .interaction(InteractionCommand::Modality(InputModality::Pointer));
    tree.drain_commands();
    assert_eq!(colors(&tree), vec![Oklch::TRANSPARENT, Oklch::WHITE]);
    tree.queue_interaction_event(root, InteractionEvent::Hover(false));
    tree.drain_commands();
    assert_eq!(colors(&tree), vec![Oklch::BLACK, Oklch::WHITE]);
}

#[test]
fn raw_hover_color_inherits_and_preserves_descendant_overrides() {
    let base = GpuiHsla {
        h: 0.1,
        s: 0.2,
        l: 0.3,
        a: 0.4,
    };
    let hover = GpuiHsla {
        h: 0.5,
        s: 0.6,
        l: 0.7,
        a: 0.8,
    };
    let explicit = GpuiHsla {
        h: 0.9,
        s: 0.8,
        l: 0.7,
        a: 0.6,
    };
    let components = |color: GpuiHsla| [color.h, color.s, color.l, color.a];
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let element = Box::new()
        .child(Text::new("inherited").build())
        .child(Text::new("explicit").gpui_color(explicit).build())
        .child(Text::new("legacy").color(Oklch::BLACK).build())
        .build()
        .style(Style::new().gpui_text_color(base))
        .hover_style(Style::new().gpui_text_color(hover));
    let root = view.set_content(&mut tree, vec![element]).unwrap()[0];
    tree.drain_commands();
    let children = tree.get(root).unwrap().children.clone();
    let colors = |tree: &UiTree| {
        children
            .iter()
            .map(|node| match &tree.get(*node).unwrap().kind {
                NodeKind::Text(text) => text.text_style.gpui_color,
                _ => panic!("expected text"),
            })
            .collect::<Vec<_>>()
    };
    let expected = |inherited| {
        vec![
            Some(components(inherited)),
            Some(components(explicit)),
            None,
        ]
    };
    assert_eq!(colors(&tree), expected(base));
    tree.queue_interaction_event(root, InteractionEvent::Hover(true));
    tree.drain_commands();
    assert_eq!(colors(&tree), expected(hover));
    tree.command_sink()
        .interaction(InteractionCommand::Modality(InputModality::Keyboard));
    tree.drain_commands();
    assert_eq!(colors(&tree), expected(base));
    tree.command_sink()
        .interaction(InteractionCommand::Modality(InputModality::Pointer));
    tree.drain_commands();
    assert_eq!(colors(&tree), expected(hover));
    tree.queue_interaction_event(root, InteractionEvent::Hover(false));
    tree.drain_commands();
    assert_eq!(colors(&tree), expected(base));
}

#[test]
fn legacy_hover_color_temporarily_clears_inherited_raw_color() {
    let base = GpuiHsla {
        h: 0.1,
        s: 0.2,
        l: 0.3,
        a: 0.4,
    };
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let element = Box::new()
        .child(Text::new("inherited").build())
        .build()
        .style(Style::new().gpui_text_color(base))
        .hover_style(Style::new().text_color(Oklch::BLACK));
    let root = view.set_content(&mut tree, vec![element]).unwrap()[0];
    tree.drain_commands();
    let child = tree.get(root).unwrap().children[0];
    let color = |tree: &UiTree| match &tree.get(child).unwrap().kind {
        NodeKind::Text(text) => (text.color, text.text_style.gpui_color),
        _ => panic!("expected text"),
    };
    assert_eq!(color(&tree).1, Some([base.h, base.s, base.l, base.a]));
    tree.queue_interaction_event(root, InteractionEvent::Hover(true));
    tree.drain_commands();
    assert_eq!(color(&tree), (Oklch::BLACK, None));
    tree.queue_interaction_event(root, InteractionEvent::Hover(false));
    tree.drain_commands();
    assert_eq!(color(&tree).1, Some([base.h, base.s, base.l, base.a]));
}

#[test]
fn source_hover_color_restores_inherited_and_explicit_color_representations() {
    use aqua_core::GpuiTextColor;

    let base = GpuiTextColor {
        hue_degrees: 330.0,
        saturation: 0.8,
        lightness: 0.4,
        alpha: 0.7,
    };
    let hover = GpuiTextColor {
        hue_degrees: -45.0,
        saturation: 0.5,
        lightness: 0.6,
        alpha: 0.9,
    };
    let explicit = GpuiTextColor {
        hue_degrees: 720.0,
        saturation: 0.3,
        lightness: 0.2,
        alpha: 1.0,
    };
    let normalized = GpuiHsla {
        h: 0.25,
        s: 0.4,
        l: 0.6,
        a: 0.8,
    };
    let array = |c: GpuiTextColor| [c.hue_degrees, c.saturation, c.lightness, c.alpha];
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let element = Box::new()
        .child(Text::new("inherited").build())
        .child(Text::new("source").gpui_source_color(explicit).build())
        .child(Text::new("normalized").gpui_color(normalized).build())
        .child(Text::new("legacy").color(Oklch::BLACK).build())
        .build()
        .style(Style::new().gpui_source_text_color(base))
        .hover_style(Style::new().gpui_source_text_color(hover));
    let root = view.set_content(&mut tree, vec![element]).unwrap()[0];
    tree.drain_commands();
    let children = tree.get(root).unwrap().children.clone();
    let colors = |tree: &UiTree| {
        children
            .iter()
            .map(|node| match &tree.get(*node).unwrap().kind {
                NodeKind::Text(text) => (
                    text.text_style.gpui_source_color,
                    text.text_style.gpui_color,
                    text.color,
                ),
                _ => panic!("expected text"),
            })
            .collect::<Vec<_>>()
    };
    let initial = colors(&tree);
    assert_eq!(initial[0].0, Some(array(base)));
    assert_eq!(initial[1].0, Some(array(explicit)));
    assert_eq!(initial[2].0, None);
    assert_eq!(
        initial[2].1,
        Some([normalized.h, normalized.s, normalized.l, normalized.a])
    );
    assert_eq!(initial[3].0, None);
    assert_eq!(initial[3].1, None);
    assert_eq!(initial[3].2, Oklch::BLACK);
    tree.queue_interaction_event(root, InteractionEvent::Hover(true));
    tree.drain_commands();
    let active = colors(&tree);
    assert_eq!(active[0].0, Some(array(hover)));
    assert_eq!(&active[1..], &initial[1..]);
    tree.command_sink()
        .interaction(InteractionCommand::Modality(InputModality::Keyboard));
    tree.drain_commands();
    assert_eq!(colors(&tree), initial);
    tree.command_sink()
        .interaction(InteractionCommand::Modality(InputModality::Pointer));
    tree.drain_commands();
    assert_eq!(colors(&tree), active);
    tree.queue_interaction_event(root, InteractionEvent::Hover(false));
    tree.drain_commands();
    assert_eq!(colors(&tree), initial);
}

#[test]
fn normalized_hover_temporarily_replaces_source_color() {
    use aqua_core::GpuiTextColor;

    let base = GpuiTextColor {
        hue_degrees: 270.0,
        saturation: 0.3,
        lightness: 0.4,
        alpha: 1.0,
    };
    let hover = GpuiHsla {
        h: 0.2,
        s: 0.4,
        l: 0.6,
        a: 0.8,
    };
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let element = Box::new()
        .child(Text::new("inherited").build())
        .build()
        .style(Style::new().gpui_source_text_color(base))
        .hover_style(Style::new().gpui_text_color(hover));
    let root = view.set_content(&mut tree, vec![element]).unwrap()[0];
    tree.drain_commands();
    let child = tree.get(root).unwrap().children[0];
    let color = |tree: &UiTree| match &tree.get(child).unwrap().kind {
        NodeKind::Text(text) => (
            text.text_style.gpui_source_color,
            text.text_style.gpui_color,
        ),
        _ => panic!("expected text"),
    };
    let original = color(&tree);
    assert_eq!(
        original.0,
        Some([
            base.hue_degrees,
            base.saturation,
            base.lightness,
            base.alpha
        ])
    );
    tree.queue_interaction_event(root, InteractionEvent::Hover(true));
    tree.drain_commands();
    assert_eq!(
        color(&tree),
        (None, Some([hover.h, hover.s, hover.l, hover.a]))
    );
    tree.queue_interaction_event(root, InteractionEvent::Hover(false));
    tree.drain_commands();
    assert_eq!(color(&tree), original);
}
