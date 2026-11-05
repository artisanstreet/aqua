use aqua_core::Oklch;
use aqua_graph::{Extent, PaintCommand};

use super::assets::{ATLAS_EXTENT, PHOTO_EXTENT};
use super::cases::{
    Case, backdrop_blur, build, clip, glyph_run, gradient, hex, photo, quad, rect, shadow,
};

pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound.max(1)
    }
}

const PALETTE: [&str; 8] = [
    "#dd0030", "#20c090", "#4060e0", "#ece030", "#a020e0", "#ff8000", "#102030", "#f0f0f0",
];

fn random_quad(rng: &mut SplitMix64, extent: Extent) -> PaintCommand {
    let (width, height) = (extent.width as f32, extent.height as f32);
    let w = rng.range(1.0, width * 0.8);
    let h = rng.range(1.0, height * 0.8);
    let x = rng.range(-w * 0.25, width - w * 0.75);
    let y = rng.range(-h * 0.25, height - h * 0.75);
    let half = (w.min(h)) * 0.5;
    let radius = match rng.below(3) {
        0 => 0.0,
        1 => rng.range(0.0, half),
        _ => half,
    };
    let border = match rng.below(3) {
        0 => 0.0,
        _ => rng.range(0.5, half.max(0.5)),
    };
    let color = PALETTE[rng.below(PALETTE.len() as u64) as usize];
    let alpha = match rng.below(3) {
        0 => 1.0,
        _ => rng.range(0.1, 1.0),
    };
    quad(x, y, w, h, color, alpha, radius, border)
}

fn random_clip(rng: &mut SplitMix64, extent: Extent) -> PaintCommand {
    let (width, height) = (extent.width as f32, extent.height as f32);
    let w = rng.range(width * 0.2, width);
    let h = rng.range(height * 0.2, height);
    clip(
        rng.range(0.0, width - w * 0.5),
        rng.range(0.0, height - h * 0.5),
        w,
        h,
    )
}

#[derive(Clone, Copy)]
enum Scope {
    Clip,
    Opacity,
    ContentBlur,
}

fn random_rect(rng: &mut SplitMix64, extent: Extent) -> aqua_core::geometry::Rect {
    let (width, height) = (extent.width as f32, extent.height as f32);
    let w = rng.range(4.0, width * 0.9);
    let h = rng.range(4.0, height * 0.9);
    rect(
        rng.range(-4.0, width - w * 0.5),
        rng.range(-4.0, height - h * 0.5),
        w,
        h,
    )
}

fn random_effect(rng: &mut SplitMix64, extent: Extent) -> PaintCommand {
    let bounds = random_rect(rng, extent);
    let color = PALETTE[rng.below(PALETTE.len() as u64) as usize];
    match rng.below(5) {
        0 => shadow(
            bounds,
            rng.range(0.0, bounds.w.min(bounds.h) * 0.5),
            [rng.range(-6.0, 6.0), rng.range(-6.0, 6.0)],
            rng.range(0.0, 8.0),
            rng.range(-3.0, 4.0),
            color,
            rng.range(0.2, 1.0),
        ),
        1 => gradient(
            bounds.x,
            bounds.y,
            bounds.w,
            bounds.h,
            (color, rng.range(0.3, 1.0)),
            (
                PALETTE[rng.below(PALETTE.len() as u64) as usize],
                rng.range(0.3, 1.0),
            ),
            (rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)),
            rng.range(0.0, bounds.w.min(bounds.h) * 0.5),
            rng.range(0.0, 3.0),
        ),
        2 => {
            let cell = rng.below(8) as f32;
            let column = (cell % 4.0) * 16.0;
            let row = (cell / 4.0).floor() * 16.0;
            let x = bounds.x.round();
            let y = bounds.y.round();
            glyph_run(&[(
                rect(x, y, 16.0, 16.0),
                rect(
                    column.min(ATLAS_EXTENT.width as f32 - 16.0),
                    row.min(ATLAS_EXTENT.height as f32 - 16.0),
                    16.0,
                    16.0,
                ),
                color,
                rng.range(0.4, 1.0),
            )])
        }
        3 => photo(
            bounds,
            rect(
                0.0,
                0.0,
                PHOTO_EXTENT.width as f32,
                PHOTO_EXTENT.height as f32,
            ),
            rng.range(0.0, bounds.w.min(bounds.h) * 0.5),
        ),
        _ => backdrop_blur(
            bounds,
            rng.range(0.5, 14.0),
            rng.range(0.0, bounds.w.min(bounds.h) * 0.5),
        ),
    }
}

fn random_commands(rng: &mut SplitMix64, extent: Extent, budget: usize) -> Vec<PaintCommand> {
    let mut commands = Vec::new();
    let mut open: Vec<Scope> = Vec::new();
    for _ in 0..budget {
        match (rng.below(12), open.len()) {
            (0..=3, _) => commands.push(random_quad(rng, extent)),
            (4 | 5, _) => commands.push(random_effect(rng, extent)),
            (6, depth) if depth < 4 => {
                commands.push(random_clip(rng, extent));
                open.push(Scope::Clip);
            }
            (9, depth) if depth < 4 => {
                commands.push(PaintCommand::PushContentBlur {
                    bounds: random_rect(rng, extent),
                    radius: rng.range(0.5, 10.0),
                });
                open.push(Scope::ContentBlur);
            }
            (7 | 8, depth) if depth < 4 => {
                commands.push(PaintCommand::PushOpacity(rng.range(0.05, 1.0)));
                open.push(Scope::Opacity);
            }
            (10 | 11, depth) if depth > 0 => {
                commands.push(match open.pop() {
                    Some(Scope::Clip) | None => PaintCommand::PopClip,
                    Some(Scope::Opacity) => PaintCommand::PopOpacity,
                    Some(Scope::ContentBlur) => PaintCommand::PopContentBlur,
                });
            }
            _ => commands.push(random_quad(rng, extent)),
        }
    }
    while let Some(scope) = open.pop() {
        commands.push(match scope {
            Scope::Clip => PaintCommand::PopClip,
            Scope::Opacity => PaintCommand::PopOpacity,
            Scope::ContentBlur => PaintCommand::PopContentBlur,
        });
    }
    commands
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub name: String,
    pub extent: Extent,
    pub clear: Oklch,
    pub commands: Vec<PaintCommand>,
}

pub fn fuzz_scenes(seed: u64, count: usize) -> Vec<Scene> {
    let extents = [
        Extent {
            width: 64,
            height: 48,
        },
        Extent {
            width: 97,
            height: 61,
        },
        Extent {
            width: 128,
            height: 40,
        },
    ];
    (0..count)
        .map(|index| {
            let mut rng =
                SplitMix64::new(seed ^ (index as u64).wrapping_mul(0xA24B_AED4_963E_E407));
            let extent = extents[rng.below(extents.len() as u64) as usize];
            let budget = 4 + rng.below(28) as usize;
            let clear = match rng.below(3) {
                0 => Oklch::TRANSPARENT,
                1 => hex("#202830", 1.0),
                _ => hex("#202830", 0.5),
            };
            Scene {
                name: format!("fuzz_{seed:x}_{index}"),
                extent,
                clear,
                commands: random_commands(&mut rng, extent, budget),
            }
        })
        .collect()
}

pub fn fuzz_cases(seed: u64, count: usize) -> Vec<Case> {
    fuzz_scenes(seed, count)
        .into_iter()
        .map(|scene| build(scene.name, scene.extent, scene.clear, &scene.commands))
        .collect()
}
