use aqua_protocol::SurfacePixels;

use super::cases::{hex, quad, standard, Case};

pub const NEAR_BLACK: u8 = 0x11;
pub const COMPOSER_GLASS_ALPHA: f32 = 0.05;
pub const GLASS_FACE_ALPHA: f32 = 0.20;
pub const TOLERANCE: u8 = 2;

pub struct Probe {
    pub x: usize,
    pub y: usize,
    pub alpha: f32,
}

pub const PROBES: [Probe; 2] = [
    Probe {
        x: 24,
        y: 36,
        alpha: COMPOSER_GLASS_ALPHA,
    },
    Probe {
        x: 72,
        y: 36,
        alpha: GLASS_FACE_ALPHA,
    },
];

pub fn gpui_over(source: u8, alpha: f32, backdrop: u8) -> u8 {
    let blended = f32::from(source) / 255.0 * alpha + f32::from(backdrop) / 255.0 * (1.0 - alpha);
    (blended * 255.0).round() as u8
}

pub fn translucent_white_case() -> Case {
    standard(
        "gpui_translucent_white_over_near_black",
        hex("#111111", 1.0),
        vec![
            quad(
                4.0,
                4.0,
                40.0,
                64.0,
                "#ffffff",
                COMPOSER_GLASS_ALPHA,
                0.0,
                0.0,
            ),
            quad(52.0, 4.0, 40.0, 64.0, "#ffffff", GLASS_FACE_ALPHA, 0.0, 0.0),
        ],
    )
}

pub fn deviation(pixels: &SurfacePixels) -> Option<(usize, usize, u8, u8)> {
    let width = pixels.extent.width as usize;
    PROBES.iter().find_map(|probe| {
        let expected = gpui_over(255, probe.alpha, NEAR_BLACK);
        let at = (probe.y * width + probe.x) * 4;
        pixels.rgba8[at..at + 3]
            .iter()
            .find(|&&actual| actual.abs_diff(expected) > TOLERANCE)
            .map(|&actual| (probe.x, probe.y, expected, actual))
    })
}
