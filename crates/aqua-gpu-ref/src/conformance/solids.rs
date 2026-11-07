use aqua_core::{Oklch, Rect};
use aqua_graph::{Extent, PassKind, SolidInstance};

use super::cases::{build, Case};

pub fn cases() -> Vec<Case> {
    [1.0, 1.25, 1.5, 2.0]
        .into_iter()
        .map(|scale| {
            let mut case = build(
                format!("solid_boxes_scale_{scale}"),
                Extent::new(128, 96),
                Oklch::TRANSPARENT,
                &[],
            );
            let instances = (0..48)
                .flat_map(|y| {
                    (0..64).map(move |x| SolidInstance {
                        bounds: Rect {
                            x: (x as f32 + 0.2) * scale,
                            y: (y as f32 + 0.3) * scale,
                            w: scale,
                            h: scale,
                        },
                        fill: Oklch::from_srgb8(
                            (x * 73) as u8,
                            (y * 39) as u8,
                            (x * y * 17) as u8,
                            if x % 3 == 0 { 0.4 } else { 1.0 },
                        ),
                    })
                })
                .collect();
            case.plan.passes[0].kind = PassKind::Solids { instances };
            case
        })
        .collect()
}
