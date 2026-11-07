use aqua_core::{Oklch, Rect, Srgba8};
use aqua_graph::{Extent, PassKind, SolidColor, SolidInstance};

use super::cases::{build, Case};

#[derive(Clone, Copy, Debug)]
enum Palette {
    Oklch,
    Srgba,
    Mixed,
}

pub fn cases() -> Vec<Case> {
    [Palette::Oklch, Palette::Srgba, Palette::Mixed]
        .into_iter()
        .flat_map(|palette| {
            [1.0, 1.25, 1.5, 2.0].into_iter().map(move |scale| {
                let mut case = build(
                    format!("solid_boxes_{palette:?}_scale_{scale}"),
                    Extent::new(128, 96),
                    Oklch::TRANSPARENT,
                    &[],
                );
                let instances = (0..48)
                    .flat_map(|y| {
                        (0..64).map(move |x| {
                            let color = Srgba8::new(
                                (x * 73) as u8,
                                (y * 39) as u8,
                                (x * y * 17) as u8,
                                if x % 3 == 0 { 102 } else { 255 },
                            );
                            let fill = match palette {
                                Palette::Oklch => SolidColor::Oklch(color.to_oklch()),
                                Palette::Srgba => SolidColor::Srgba(color),
                                Palette::Mixed if x % 2 == 0 => SolidColor::Srgba(color),
                                Palette::Mixed => SolidColor::Oklch(color.to_oklch()),
                            };
                            SolidInstance {
                                bounds: Rect {
                                    x: (x as f32 + 0.2) * scale,
                                    y: (y as f32 + 0.3) * scale,
                                    w: scale,
                                    h: scale,
                                },
                                fill,
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                case.plan.passes[0].kind = match palette {
                    Palette::Srgba => {
                        let mut bounds = Vec::new();
                        let mut fills = Vec::new();
                        for instance in instances {
                            let SolidColor::Srgba(fill) = instance.fill else {
                                unreachable!()
                            };
                            bounds.push(instance.bounds);
                            fills.push(fill);
                        }
                        PassKind::SrgbaSolids {
                            bounds: bounds.into(),
                            fills,
                        }
                    }
                    _ => PassKind::Solids { instances },
                };
                case
            })
        })
        .collect()
}

pub fn retained<B: aqua_protocol::GpuBackend + aqua_protocol::GpuReadback>(
    backend: &mut B,
) -> Result<(), String> {
    let mut reference = crate::RefBackend::new();
    let mut reference = super::Session::new(&mut reference);
    let mut candidate = super::Session::new(backend);
    let original: std::sync::Arc<[Rect]> = vec![Rect {
        x: 3.25,
        y: 4.5,
        w: 12.75,
        h: 11.25,
    }]
    .into();
    let moved: std::sync::Arc<[Rect]> = vec![Rect {
        x: 23.5,
        y: 14.25,
        w: 9.25,
        h: 12.5,
    }]
    .into();
    let mut case = build(
        "retained_srgba",
        Extent::new(48, 40),
        Oklch::TRANSPARENT,
        &[],
    );
    for frame in 0..6 {
        let bounds = match frame {
            2 | 3 => &moved,
            _ => &original,
        };
        case.plan.passes[0].kind = PassKind::SrgbaSolids {
            bounds: std::sync::Arc::clone(bounds),
            fills: vec![Srgba8::new(
                (frame * 43) as u8,
                141,
                235,
                if frame % 2 == 0 { 255 } else { 102 },
            )],
        };
        let expected = reference
            .render(&case.plan)
            .map_err(|error| error.to_string())?;
        let actual = candidate
            .render(&case.plan)
            .map_err(|error| error.to_string())?;
        let comparison =
            super::compare(&case.plan, &expected, &actual, super::Tolerance::default());
        if !comparison.passed() {
            return Err(format!("frame {frame}: {comparison:?}"));
        }
    }
    Ok(())
}
