use aqua_core::scene::GpuiTextOpacityRange;
use aqua_ui::element::{lower_specs, LowerError, Text};

#[test]
fn lowering_rejects_invalid_opacity_ranges() {
    let range = |bytes, opacity| GpuiTextOpacityRange { bytes, opacity };
    let invalid = vec![
        vec![range(1..2, 0.5)],
        vec![range(0..4, 0.5), range(2..5, 0.5)],
        vec![range(0..7, 0.5)],
        vec![range(0..2, f32::NAN)],
        vec![range(0..2, 1.1)],
        vec![range(2..2, 0.5)],
    ];
    for ranges in invalid {
        let element = Text::new("é abc").gpui_text_opacity(ranges).build();
        assert!(matches!(
            lower_specs(&element),
            Err(LowerError::InvalidTextOpacity)
        ));
    }
    let element = Text::new("é abc")
        .gpui_text_opacity(vec![range(0..2, 0.25), range(3..6, 1.0)])
        .build();
    assert!(lower_specs(&element).is_ok());
}
