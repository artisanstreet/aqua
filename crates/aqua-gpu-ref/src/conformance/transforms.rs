use aqua_core::Oklch;
use aqua_graph::{Gradient, GradientQuad, GradientStop, Image, Linear2, PaintCommand, Quad};

use super::assets::PHOTO;
use super::cases::{backdrop, hex, quad, rect, standard, Case};

fn linear(matrix: Linear2, leaf: PaintCommand) -> PaintCommand {
    PaintCommand::Linear {
        linear: matrix,
        leaf: Box::new(leaf),
    }
}

fn bordered(x: f32, y: f32, w: f32, h: f32, color: &str, radius: f32, border: f32) -> PaintCommand {
    quad(x, y, w, h, color, 0.9, radius, border)
}

fn gradient_quad(x: f32, y: f32, w: f32, h: f32, radius: f32) -> PaintCommand {
    PaintCommand::GradientQuad(GradientQuad {
        quad: Quad {
            bounds: rect(x, y, w, h),
            radius,
            border_width: 0.0,
            fill: Oklch::TRANSPARENT,
            border_color: hex("#e09030", 0.7),
        },
        gradient: Gradient {
            start: [x, y],
            end: [x + w, y + h],
            stops: [
                GradientStop {
                    offset: 0.0,
                    color: hex("#20c0e0", 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: hex("#e04080", 0.8),
                },
            ],
        },
    })
}

fn photo_at(x: f32, y: f32, w: f32, h: f32, radius: f32) -> PaintCommand {
    PaintCommand::Image(Image {
        source: PHOTO,
        bounds: rect(x, y, w, h),
        texels: rect(0.0, 0.0, 32.0, 32.0),
        radius,
    })
}

pub fn cases() -> Vec<Case> {
    vec![
        standard(
            "quad_rotated_rounded_bordered",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                linear(
                    Linear2::rotation(0.45),
                    bordered(14.0, 14.0, 44.0, 28.0, "#dd0030", 6.0, 3.0),
                ),
                linear(
                    Linear2::rotation(-1.2),
                    bordered(58.0, 30.0, 30.0, 30.0, "#20c090", 0.0, 0.0),
                ),
            ],
        ),
        standard(
            "quad_skewed_and_scaled",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                linear(
                    Linear2::skew(0.5, 0.0),
                    bordered(10.0, 12.0, 40.0, 24.0, "#ece030", 5.0, 0.0),
                ),
                linear(
                    Linear2::scale(0.6, 1.4).then(Linear2::rotation(0.3)),
                    bordered(52.0, 26.0, 32.0, 26.0, "#4060e0", 4.0, 2.0),
                ),
            ],
        ),
        standard(
            "quad_gradient_rotated",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                linear(
                    Linear2::rotation(0.7),
                    gradient_quad(20.0, 18.0, 50.0, 30.0, 8.0),
                ),
            ],
        ),
        standard(
            "quad_rotated_clipped_in_group",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                PaintCommand::PushClip(rect(16.0, 10.0, 56.0, 40.0)),
                PaintCommand::PushOpacity(0.7),
                linear(
                    Linear2::rotation(0.35),
                    bordered(10.0, 14.0, 60.0, 30.0, "#ff8000", 3.0, 0.0),
                ),
                linear(
                    Linear2::rotation(-0.35),
                    bordered(30.0, 20.0, 50.0, 30.0, "#4060e0", 3.0, 0.0),
                ),
                PaintCommand::PopOpacity,
                PaintCommand::PopClip,
            ],
        ),
        standard(
            "image_rotated_rounded",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                linear(
                    Linear2::rotation(0.6),
                    photo_at(20.0, 14.0, 36.0, 36.0, 8.0),
                ),
            ],
        ),
        standard(
            "image_skewed_scaled",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                linear(
                    Linear2::skew(-0.4, 0.15).then(Linear2::scale(1.3, 0.8)),
                    photo_at(14.0, 16.0, 30.0, 30.0, 0.0),
                ),
                linear(
                    Linear2::rotation(1.5707964),
                    photo_at(60.0, 20.0, 24.0, 24.0, 4.0),
                ),
            ],
        ),
    ]
}
