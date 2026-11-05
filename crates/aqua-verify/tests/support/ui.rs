#![allow(dead_code)]

use aqua::UiApp;
use aqua_core::Oklch;
use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
use aqua_protocol::window::ObservedWindowState;
use aqua_ui::element::ui::{Box as UiBox, Scroll, Text as UiText};
use aqua_ui::style::{px, Axis, Style};

pub const ROW: f32 = 64.0;
pub const ROWS: usize = 400;

pub fn observed(width: f32, height: f32) -> ObservedWindowState {
    ObservedWindowState {
        id: AquaWindowId::new(1).unwrap(),
        title: "aqua-verify".into(),
        inner_size: LogicalSize::new(f64::from(width), f64::from(height)),
        scale_factor: 1.0,
        focused: true,
        visible: true,
        occluded: false,
        maximized: false,
    }
}

pub fn editor_like(app: &mut UiApp, width: f32, height: f32) {
    {
        app.mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Row))
                .child(
                    UiBox::new()
                        .fill(Oklch::from_hex("#181825").unwrap())
                        .style(Style::new().width(px(120.0)).height(px(height)))
                        .build(),
                )
                .child(
                    Scroll::new()
                        .style(Style::new().width(px(width - 120.0)).height(px(height)))
                        .child(
                            UiBox::new()
                                .style(
                                    Style::new()
                                        .axis(Axis::Column)
                                        .width(px(width - 120.0))
                                        .height(px(ROW * ROWS as f32))
                                        .flex_shrink(0.0),
                                )
                                .children((0..ROWS).map(|row| {
                                    let fill = match row % 2 {
                                        0 => "#223344",
                                        _ => "#445566",
                                    };
                                    UiBox::new()
                                        .fill(Oklch::from_hex(fill).unwrap())
                                        .style(
                                            Style::new()
                                                .width(px(width - 120.0))
                                                .height(px(ROW))
                                                .flex_shrink(0.0),
                                        )
                                        .child(
                                            UiText::builder()
                                                .arg(format!(
                                                    "fn line_{row}() {{ let value = {row}; }}"
                                                ))
                                                .style(Style::new().width(px(400.0)))
                                                .build(),
                                        )
                                        .build()
                                }))
                                .build(),
                        )
                        .build(),
                )
                .build()
        })
        .unwrap();
    }
}
