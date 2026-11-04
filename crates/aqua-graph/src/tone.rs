// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

const RATIOS: [[f32; 4]; 13] = [
    [0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0],
    [0.0166 / 4.0, -0.0807 / 4.0, 0.2227 / 4.0, -0.0751 / 4.0],
    [0.0350 / 4.0, -0.1760 / 4.0, 0.4325 / 4.0, -0.1370 / 4.0],
    [0.0543 / 4.0, -0.2821 / 4.0, 0.6302 / 4.0, -0.1876 / 4.0],
    [0.0739 / 4.0, -0.3963 / 4.0, 0.8167 / 4.0, -0.2287 / 4.0],
    [0.0933 / 4.0, -0.5161 / 4.0, 0.9926 / 4.0, -0.2616 / 4.0],
    [0.1121 / 4.0, -0.6395 / 4.0, 1.1588 / 4.0, -0.2877 / 4.0],
    [0.1300 / 4.0, -0.7649 / 4.0, 1.3159 / 4.0, -0.3080 / 4.0],
    [0.1469 / 4.0, -0.8911 / 4.0, 1.4644 / 4.0, -0.3234 / 4.0],
    [0.1627 / 4.0, -1.0170 / 4.0, 1.6051 / 4.0, -0.3347 / 4.0],
    [0.1773 / 4.0, -1.1420 / 4.0, 1.7385 / 4.0, -0.3426 / 4.0],
    [0.1908 / 4.0, -1.2652 / 4.0, 1.8650 / 4.0, -0.3476 / 4.0],
    [0.2031 / 4.0, -1.3864 / 4.0, 1.9851 / 4.0, -0.3501 / 4.0],
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneCurve {
    ratios: [f32; 4],
    contrast: f32,
}

impl ToneCurve {
    pub const EXACT: ToneCurve = ToneCurve {
        ratios: [0.0; 4],
        contrast: 0.0,
    };

    pub fn new(gamma: f32, contrast: f32) -> Self {
        const NORM13: f32 = ((0x10000 as f64) / (255.0 * 255.0) * 4.0) as f32;
        const NORM24: f32 = ((0x100 as f64) / 255.0 * 4.0) as f32;
        let index = ((gamma * 10.0).round() as usize).clamp(10, 22) - 10;
        let ratio = RATIOS[index];
        Self {
            ratios: [
                ratio[0] * NORM13,
                ratio[1] * NORM24,
                ratio[2] * NORM13,
                ratio[3] * NORM24,
            ],
            contrast: contrast.max(0.0),
        }
    }

    pub fn native() -> Self {
        Self::new(1.8, 1.0)
    }

    pub fn ratios(&self) -> [f32; 4] {
        self.ratios
    }

    pub fn contrast(&self) -> f32 {
        self.contrast
    }

    pub fn adjust(&self, coverage: f32, luminance: f32) -> f32 {
        let boost = self.contrast * (4.0 * (0.75 - luminance)).clamp(0.0, 1.0);
        let alpha = coverage * (boost + 1.0) / (coverage * boost + 1.0);
        let level = self.ratios[0] * luminance + self.ratios[1];
        let offset = self.ratios[2] * luminance + self.ratios[3];
        alpha + alpha * (1.0 - alpha) * (level * alpha + offset)
    }

    pub fn linear_blend_alpha(&self, coverage: f32, luminance: f32) -> f32 {
        let backdrop = if luminance > 0.5 { 0.0 } else { 1.0 };
        self.linear_blend_alpha_over(coverage, luminance, backdrop)
    }

    pub fn linear_blend_alpha_over(&self, coverage: f32, luminance: f32, backdrop: f32) -> f32 {
        let toned = self.adjust(coverage, luminance).clamp(0.0, 1.0);
        let encoded = backdrop + (luminance - backdrop) * toned;
        let (text, behind, blended) = (
            srgb_to_linear(luminance),
            srgb_to_linear(backdrop),
            srgb_to_linear(encoded),
        );
        match (text - behind).abs() > 1.0e-4 {
            true => ((blended - behind) / (text - behind)).clamp(0.0, 1.0),
            false => toned,
        }
    }

    pub fn table(&self, luminance: f32) -> [u8; 256] {
        std::array::from_fn(|index| {
            let adjusted = self.adjust(index as f32 / 255.0, luminance);
            (adjusted.clamp(0.0, 1.0) * 255.0).round() as u8
        })
    }
}

pub fn luminance(srgb: [f32; 3]) -> f32 {
    srgb[0] * 0.30 + srgb[1] * 0.59 + srgb[2] * 0.11
}

pub fn srgb_to_linear(encoded: f32) -> f32 {
    let value = encoded.clamp(0.0, 1.0);
    match value <= 0.040_45 {
        true => value / 12.92,
        false => ((value + 0.055) / 1.055).powf(2.4),
    }
}

pub fn linear_to_srgb(linear: f32) -> f32 {
    let value = linear.clamp(0.0, 1.0);
    match value <= 0.003_130_8 {
        true => value * 12.92,
        false => 1.055 * value.powf(1.0 / 2.4) - 0.055,
    }
}

pub fn premultiplied_linear_luminance(color: [f32; 4]) -> f32 {
    let alpha = color[3];
    match alpha > 0.0 {
        true => luminance([
            linear_to_srgb(color[0] / alpha),
            linear_to_srgb(color[1] / alpha),
            linear_to_srgb(color[2] / alpha),
        ]),
        false => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_curve_is_identity() {
        for luminance in [0.0, 0.5, 1.0] {
            for coverage in [0.0, 0.25, 0.5, 1.0] {
                assert!((ToneCurve::EXACT.adjust(coverage, luminance) - coverage).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn tables_keep_endpoints_and_stay_monotonic() {
        for (gamma, contrast) in [(1.8, 0.5), (2.2, 1.0), (1.0, 0.0)] {
            for luminance in [0.0, 0.2, 0.5, 0.8, 1.0] {
                let table = ToneCurve::new(gamma, contrast).table(luminance);
                assert_eq!(table[0], 0);
                assert_eq!(table[255], 255);
                assert!(
                    table.windows(2).all(|pair| pair[0] <= pair[1]),
                    "{gamma} {contrast} {luminance}"
                );
            }
        }
    }

    #[test]
    fn linear_blend_alpha_keeps_endpoints_and_lifts_dark_text() {
        let curve = ToneCurve::native();
        for luminance in [0.0, 0.3, 0.7, 1.0] {
            assert!(curve.linear_blend_alpha(0.0, luminance).abs() < 1e-5);
            assert!((curve.linear_blend_alpha(1.0, luminance) - 1.0).abs() < 1e-4);
        }
        assert!(curve.linear_blend_alpha(0.5, 0.0) > curve.adjust(0.5, 0.0));
        assert!(curve.linear_blend_alpha(0.5, 1.0) < curve.adjust(0.5, 1.0));
    }

    #[test]
    fn light_text_is_boosted_more_than_dark_text() {
        let curve = ToneCurve::new(1.8, 1.0);
        assert!(curve.adjust(0.5, 1.0) > 0.5);
        assert!(curve.adjust(0.5, 1.0) > curve.adjust(0.5, 0.0));
    }
}
