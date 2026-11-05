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

impl Default for ToneCurve {
    fn default() -> Self {
        Self::native()
    }
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

    pub fn alpha(&self, coverage: f32, luminance: f32) -> f32 {
        self.adjust(coverage, luminance).clamp(0.0, 1.0)
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

pub fn premultiplied_luminance(color: [f32; 4]) -> f32 {
    let alpha = color[3];
    match alpha > 0.0 {
        true => luminance([color[0] / alpha, color[1] / alpha, color[2] / alpha]),
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
    fn alpha_keeps_endpoints_and_stays_in_range() {
        let curve = ToneCurve::native();
        for luminance in [0.0, 0.3, 0.7, 1.0] {
            assert!(curve.alpha(0.0, luminance).abs() < 1e-5);
            assert!((curve.alpha(1.0, luminance) - 1.0).abs() < 1e-4);
            for coverage in [0.1, 0.5, 0.9] {
                assert!((0.0..=1.0).contains(&curve.alpha(coverage, luminance)));
            }
        }
    }

    #[test]
    fn luminance_of_premultiplied_color_ignores_alpha() {
        let straight = luminance([0.8, 0.4, 0.2]);
        let premultiplied = premultiplied_luminance([0.4, 0.2, 0.1, 0.5]);
        assert!((straight - premultiplied).abs() < 1e-6);
        assert_eq!(premultiplied_luminance([0.0; 4]), 0.0);
    }

    #[test]
    fn light_text_is_boosted_more_than_dark_text() {
        let curve = ToneCurve::new(1.8, 1.0);
        assert!(curve.adjust(0.5, 1.0) > 0.5);
        assert!(curve.adjust(0.5, 1.0) > curve.adjust(0.5, 0.0));
    }
}
