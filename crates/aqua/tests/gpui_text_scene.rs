use aqua::{
    ui::{Box as UiBox, Text},
    Style, UiApp,
};
use aqua_core::{DisplayItem, GpuiHsla};
use aqua_text::{SharedFonts, SharedRuns};

#[test]
fn ui_app_emits_inherited_raw_text_and_placeholder_scopes() {
    let base = GpuiHsla {
        h: 0.12,
        s: 0.34,
        l: 0.56,
        a: 0.78,
    };
    let placeholder = GpuiHsla {
        h: 0.8,
        s: 0.7,
        l: 0.6,
        a: 0.5,
    };
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| {
        UiBox::new()
            .layout("column")
            .style(Style::new().gpui_text_color(base))
            .child(Text::new("original text").build())
            .child(
                Text::new("")
                    .editable(true)
                    .placeholder("hint")
                    .gpui_placeholder_color(placeholder)
                    .build(),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(400.0, 200.0, 1.0);
    app.build_frame().unwrap();
    let mut active = None;
    let mut colors = Vec::new();
    for (_, item) in app.scene().ordered() {
        match item {
            DisplayItem::PushGpuiTextColor(color) => active = Some(*color),
            DisplayItem::PopGpuiTextColor => active = None,
            DisplayItem::Text { .. } => colors.push(active),
            _ => {}
        }
    }
    assert_eq!(colors, vec![Some(base), Some(placeholder)]);
}
