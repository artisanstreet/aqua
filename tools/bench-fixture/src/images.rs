use anyhow::{Result, ensure};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ImageSpec {
    pub width: u32,
    pub height: u32,
    pub seed: u64,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u32) -> u32 {
        (self.next() % u64::from(bound.max(1))) as u32
    }
}

fn fill_rect(pixels: &mut [u8], width: u32, rect: (u32, u32, u32, u32), color: [u8; 3]) {
    let (x0, y0, w, h) = rect;
    for y in y0..y0 + h {
        for x in x0..(x0 + w).min(width) {
            let at = ((y * width + x) * 4) as usize;
            if at + 3 < pixels.len() {
                pixels[at..at + 3].copy_from_slice(&color);
            }
        }
    }
}

pub fn render(spec: ImageSpec) -> Result<Vec<u8>> {
    ensure!(spec.width > 0 && spec.height > 0, "empty image");
    let (width, height) = (spec.width, spec.height);
    let mut rng = Rng(spec.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let base = [
        (rng.below(120) + 20) as u8,
        (rng.below(120) + 20) as u8,
        (rng.below(120) + 40) as u8,
    ];
    let mut pixels = vec![255u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let at = ((y * width + x) * 4) as usize;
            let t = (x + y) as f32 / (width + height) as f32;
            for channel in 0..3 {
                pixels[at + channel] = (f32::from(base[channel]) + t * 90.0) as u8;
            }
        }
    }
    for _ in 0..24 {
        let w = rng.below(width / 3) + 12;
        let h = rng.below(height / 3) + 12;
        let x = rng.below(width.saturating_sub(w).max(1));
        let y = rng.below(height.saturating_sub(h).max(1));
        let color = [
            (rng.below(200) + 40) as u8,
            (rng.below(200) + 40) as u8,
            (rng.below(200) + 40) as u8,
        ];
        fill_rect(&mut pixels, width, (x, y, w, h), color);
    }
    let encoded = webp::Encoder::from_rgba(&pixels, width, height).encode_lossless();
    Ok(encoded.to_vec())
}
