use aqua_core::GpuiHsla;

#[test]
fn srgb8_channels_match_the_scalar_transfer_curve_bit_for_bit() {
    for value in 0..=255u8 {
        let color = [value, value.wrapping_mul(73), value ^ 0xa5];
        let expected = color.map(|channel| {
            let encoded = f32::from(channel) / 255.0;
            match encoded <= 0.04045 {
                true => encoded / 12.92,
                false => ((encoded + 0.055) / 1.055).powf(2.4),
            }
        });
        assert_eq!(
            aqua_core::color::srgb8_to_linear(color).map(f32::to_bits),
            expected.map(f32::to_bits),
            "color={color:?}"
        );
    }
}

#[test]
#[ignore]
fn srgb8_transfer_report() {
    let colors = (0..1_024_000u32)
        .map(|index| [index as u8, (index >> 8) as u8, (index >> 16) as u8])
        .collect::<Vec<_>>();
    std::hint::black_box(aqua_core::color::srgb8_to_linear(colors[0]));
    for sample in 1..=5 {
        let begin = std::time::Instant::now();
        let output = colors
            .iter()
            .copied()
            .map(aqua_core::color::srgb8_to_linear)
            .collect::<Vec<_>>();
        let elapsed = begin.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(output.len(), colors.len());
        let digest = output.iter().flatten().fold(0u64, |sum, value| {
            sum.wrapping_mul(31)
                .wrapping_add(u64::from(value.to_bits()))
        });
        std::hint::black_box(&output);
        println!(
            "srgb8-transfer sample={sample} colors={} ms={elapsed:.3} digest={digest:016x}",
            colors.len()
        );
    }
}

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
