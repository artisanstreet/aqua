use aqua_core::GpuiHsla;

#[test]
fn gpui_hsla_conversion_retains_float_alpha_and_fractional_channels() {
    let gray = GpuiHsla {
        h: 0.2,
        s: 0.0,
        l: 0.314159,
        a: 0.271828,
    }
    .to_srgba();
    assert_eq!(gray, [0.314159, 0.314159, 0.314159, 0.271828]);
    let red = GpuiHsla {
        h: 0.0,
        s: 1.0,
        l: 0.5,
        a: 0.75,
    }
    .to_srgba();
    assert_eq!(red, [1.0, 0.0, 0.0, 0.75]);
    let cyan = GpuiHsla {
        h: 0.5,
        s: 1.0,
        l: 0.5,
        a: 1.0,
    }
    .to_srgba();
    assert_eq!(cyan, [0.0, 1.0, 1.0, 1.0]);
}
