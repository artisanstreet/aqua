pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((bits >> 10) & 0x1f) as i32;
    let mantissa = (bits & 0x3ff) as f32;
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 if mantissa == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

pub fn unorm8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub fn rgba16f_rows_to_rgba8(
    source: &[u8],
    row_pitch: usize,
    width: usize,
    height: usize,
) -> Vec<u8> {
    (0..height)
        .flat_map(|row| {
            let start = row * row_pitch;
            source[start..start + width * 8]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| unorm8(f16_to_f32(u16::from_le_bytes([pair[0], pair[1]]))))
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn srgb_to_linear(value: f32) -> f32 {
    match value <= 0.04045 {
        true => value / 12.92,
        false => ((value + 0.055) / 1.055).powf(2.4),
    }
}

pub fn bgra8_rows_to_rgba8(
    source: &[u8],
    row_pitch: usize,
    width: usize,
    height: usize,
) -> Vec<u8> {
    (0..height)
        .flat_map(|row| {
            let start = row * row_pitch;
            source[start..start + width * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|texel| [texel[2], texel[1], texel[0], texel[3]])
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn bgra8_srgb_rows_to_linear_rgba8(
    source: &[u8],
    row_pitch: usize,
    width: usize,
    height: usize,
) -> Vec<u8> {
    let decode = |byte: u8| unorm8(srgb_to_linear(byte as f32 / 255.0));
    (0..height)
        .flat_map(|row| {
            let start = row * row_pitch;
            source[start..start + width * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|texel| {
                    [
                        decode(texel[2]),
                        decode(texel[1]),
                        decode(texel[0]),
                        texel[3],
                    ]
                })
                .collect::<Vec<_>>()
        })
        .collect()
}
