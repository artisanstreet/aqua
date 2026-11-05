use aqua_core::{Oklch, geometry::Rect};
use aqua_graph::{
    BackdropBlur, Extent, FramePlan, Glyph, GlyphRun, Gradient, GradientQuad, GradientStop, Image,
    PaintCommand, Quad, Shadow, ShadowCast, plan_paint,
};
use aqua_protocol::UploadBatch;

use super::assets::{ATLAS, PHOTO, standard_uploads};

#[derive(Clone, Debug)]
pub struct Case {
    pub name: String,
    pub plan: FramePlan,
    pub uploads: UploadBatch,
}

pub fn hex(value: &str, alpha: f32) -> Oklch {
    Oklch::from_hex(value).expect("valid hex").with_alpha(alpha)
}

#[allow(clippy::too_many_arguments)]
pub fn quad(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: &str,
    alpha: f32,
    radius: f32,
    border: f32,
) -> PaintCommand {
    PaintCommand::Quad(Quad {
        bounds: Rect { x, y, w, h },
        radius,
        border_width: border,
        fill: hex(color, alpha),
        border_color: hex("#e09030", 0.7),
    })
}

pub fn clip(x: f32, y: f32, w: f32, h: f32) -> PaintCommand {
    PaintCommand::PushClip(Rect { x, y, w, h })
}

pub const STANDARD: Extent = Extent {
    width: 96,
    height: 72,
};

pub fn build(
    name: impl Into<String>,
    extent: Extent,
    clear: Oklch,
    commands: &[PaintCommand],
) -> Case {
    let name = name.into();
    let plan = plan_paint(extent, clear, commands)
        .unwrap_or_else(|error| panic!("conformance case {name} does not plan: {error}"));
    Case {
        name,
        plan,
        uploads: standard_uploads(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn gradient(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    from: (&str, f32),
    to: (&str, f32),
    angle: (f32, f32),
    radius: f32,
    border: f32,
) -> PaintCommand {
    PaintCommand::GradientQuad(GradientQuad {
        quad: Quad {
            bounds: Rect { x, y, w, h },
            radius,
            border_width: border,
            fill: Oklch::TRANSPARENT,
            border_color: hex("#e09030", 0.7),
        },
        gradient: Gradient {
            start: [x, y],
            end: [x + w * angle.0, y + h * angle.1],
            stops: [
                GradientStop {
                    offset: 0.0,
                    color: hex(from.0, from.1),
                },
                GradientStop {
                    offset: 1.0,
                    color: hex(to.0, to.1),
                },
            ],
        },
    })
}

pub fn shadow(
    bounds: Rect,
    radius: f32,
    offset: [f32; 2],
    sigma: f32,
    spread: f32,
    color: &str,
    alpha: f32,
) -> PaintCommand {
    PaintCommand::Shadow(Shadow {
        bounds,
        radius,
        offset,
        sigma,
        spread,
        color: hex(color, alpha),
        cast: ShadowCast::Whole,
    })
}

pub fn glyph_run(glyphs: &[(Rect, Rect, &str, f32)]) -> PaintCommand {
    PaintCommand::Glyphs(GlyphRun {
        atlas: ATLAS,
        glyphs: glyphs
            .iter()
            .map(|(bounds, texels, color, alpha)| Glyph {
                bounds: *bounds,
                texels: *texels,
                color: hex(color, *alpha),
            })
            .collect(),
    })
}

pub fn photo(bounds: Rect, texels: Rect, radius: f32) -> PaintCommand {
    PaintCommand::Image(Image {
        source: PHOTO,
        bounds,
        texels,
        radius,
    })
}

pub fn backdrop_blur(bounds: Rect, radius: f32, corner_radius: f32) -> PaintCommand {
    PaintCommand::BackdropBlur(BackdropBlur {
        bounds,
        radius,
        corner_radius,
    })
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

pub fn stripes(extent: Extent) -> Vec<PaintCommand> {
    let colors = ["#dd0030", "#20c090", "#4060e0", "#ece030"];
    let mut commands = vec![quad(
        0.0,
        0.0,
        extent.width as f32,
        extent.height as f32,
        "#202830",
        1.0,
        0.0,
        0.0,
    )];
    let period = 28.0;
    let count = (extent.width as f32 / period).ceil() as usize;
    for index in 0..count {
        commands.push(quad(
            6.0 + index as f32 * period,
            6.0,
            14.0,
            extent.height as f32 - 12.0,
            colors[index % colors.len()],
            1.0,
            0.0,
            0.0,
        ));
    }
    commands
}

pub(super) fn standard(name: &str, clear: Oklch, commands: Vec<PaintCommand>) -> Case {
    build(name, STANDARD, clear, &commands)
}

pub(super) fn backdrop() -> PaintCommand {
    quad(0.0, 0.0, 96.0, 72.0, "#253040", 1.0, 0.0, 0.0)
}

fn nested_commands() -> Vec<PaintCommand> {
    vec![
        backdrop(),
        quad(8.0, 8.0, 72.0, 56.0, "#dd0030", 0.6, 0.0, 5.0),
        clip(16.0, 12.0, 68.0, 48.0),
        PaintCommand::PushOpacity(0.65),
        quad(0.0, 0.0, 56.0, 64.0, "#20c090", 0.8, 0.0, 0.0),
        PaintCommand::PushOpacity(0.45),
        quad(32.0, 4.0, 64.0, 64.0, "#4060e0", 0.9, 0.0, 0.0),
        quad(16.0, 28.0, 48.0, 32.0, "#ece030", 0.7, 0.0, 0.0),
        PaintCommand::PopOpacity,
        clip(40.0, 20.0, 32.0, 28.0),
        quad(8.0, 0.0, 88.0, 72.0, "#a020e0", 0.35, 0.0, 0.0),
        PaintCommand::PopClip,
        PaintCommand::PopOpacity,
        PaintCommand::PopClip,
        quad(4.0, 52.0, 84.0, 16.0, "#30e0b0", 0.25, 0.0, 0.0),
    ]
}

fn grid_commands() -> Vec<PaintCommand> {
    let colors = ["#dd0030", "#20c090", "#4060e0", "#ece030", "#a020e0"];
    let mut commands = vec![backdrop()];
    for row in 0..9u32 {
        for column in 0..12u32 {
            let index = (row * 12 + column) as usize;
            commands.push(quad(
                2.0 + column as f32 * 7.9,
                2.0 + row as f32 * 7.7,
                9.0,
                9.0,
                colors[index % colors.len()],
                0.35 + 0.1 * (index % 6) as f32,
                (index % 5) as f32,
                (index % 3) as f32,
            ));
        }
    }
    commands
}

fn deep_commands() -> Vec<PaintCommand> {
    let mut commands = vec![backdrop()];
    let colors = ["#dd0030", "#20c090", "#4060e0", "#ece030"];
    for (depth, color) in colors.iter().enumerate() {
        commands.push(PaintCommand::PushOpacity(0.8));
        commands.push(quad(
            4.0 + depth as f32 * 9.0,
            4.0 + depth as f32 * 6.0,
            60.0,
            44.0,
            color,
            0.9,
            4.0,
            0.0,
        ));
    }
    commands.extend(std::iter::repeat_n(PaintCommand::PopOpacity, colors.len()));
    commands
}

pub fn cases() -> Vec<Case> {
    let mut nested = nested_commands();
    let mut cases = vec![
        standard(
            "solid_rect",
            Oklch::TRANSPARENT,
            vec![quad(8.0, 8.0, 80.0, 56.0, "#ff0000", 1.0, 0.0, 0.0)],
        ),
        standard(
            "translucent_over_backdrop",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                quad(10.0, 10.0, 60.0, 40.0, "#dd0030", 0.6, 0.0, 0.0),
                quad(30.0, 20.0, 50.0, 40.0, "#20c090", 0.4, 0.0, 0.0),
            ],
        ),
        standard(
            "border_alpha",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                quad(8.0, 8.0, 72.0, 56.0, "#dd0030", 0.6, 0.0, 5.0),
            ],
        ),
        standard(
            "rounded_with_border",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                quad(8.0, 8.0, 80.0, 56.0, "#4060e0", 1.0, 12.0, 3.0),
            ],
        ),
        standard(
            "opaque_clear_color",
            hex("#102030", 1.0),
            vec![quad(20.0, 20.0, 40.0, 30.0, "#ece030", 0.5, 6.0, 0.0)],
        ),
        standard(
            "translucent_clear_color",
            hex("#102030", 0.5),
            vec![quad(20.0, 20.0, 40.0, 30.0, "#ece030", 0.5, 6.0, 0.0)],
        ),
        standard(
            "clip_intersection",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                clip(16.0, 12.0, 60.0, 48.0),
                clip(30.0, 20.0, 60.0, 30.0),
                quad(0.0, 0.0, 96.0, 72.0, "#a020e0", 0.7, 0.0, 0.0),
                PaintCommand::PopClip,
                quad(40.0, 4.0, 20.0, 10.0, "#30e0b0", 0.8, 0.0, 0.0),
                PaintCommand::PopClip,
            ],
        ),
        standard(
            "disjoint_clips_cull_everything",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                clip(0.0, 0.0, 20.0, 20.0),
                clip(40.0, 40.0, 20.0, 20.0),
                quad(0.0, 0.0, 96.0, 72.0, "#a020e0", 0.7, 0.0, 0.0),
                PaintCommand::PopClip,
                PaintCommand::PopClip,
            ],
        ),
        standard(
            "isolated_group_opacity",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                PaintCommand::PushOpacity(0.5),
                quad(10.0, 10.0, 50.0, 40.0, "#ff0000", 1.0, 0.0, 0.0),
                quad(30.0, 20.0, 50.0, 40.0, "#0000ff", 1.0, 0.0, 0.0),
                PaintCommand::PopOpacity,
            ],
        ),
        standard(
            "group_opacity_zero",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                PaintCommand::PushOpacity(0.0),
                quad(10.0, 10.0, 50.0, 40.0, "#ff0000", 1.0, 0.0, 0.0),
                PaintCommand::PopOpacity,
            ],
        ),
        standard(
            "group_opacity_one",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                PaintCommand::PushOpacity(1.0),
                quad(10.0, 10.0, 50.0, 40.0, "#ff0000", 0.5, 8.0, 2.0),
                quad(30.0, 20.0, 50.0, 40.0, "#0000ff", 0.5, 8.0, 2.0),
                PaintCommand::PopOpacity,
            ],
        ),
        standard(
            "empty_group_and_empty_frame",
            Oklch::TRANSPARENT,
            vec![PaintCommand::PushOpacity(0.5), PaintCommand::PopOpacity],
        ),
        standard(
            "fractional_edges",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                quad(10.25, 10.5, 40.5, 30.25, "#ece030", 1.0, 0.0, 0.0),
                quad(20.75, 24.125, 50.5, 30.75, "#4060e0", 0.7, 5.5, 1.5),
            ],
        ),
        standard(
            "pill_and_circle",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                quad(6.0, 6.0, 60.0, 20.0, "#20c090", 1.0, 10.0, 0.0),
                quad(10.0, 34.0, 32.0, 32.0, "#dd0030", 1.0, 16.0, 4.0),
                quad(50.0, 34.0, 32.0, 32.0, "#a020e0", 0.6, 16.0, 0.0),
            ],
        ),
        standard(
            "border_fills_quad",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                quad(10.0, 10.0, 40.0, 40.0, "#4060e0", 1.0, 0.0, 20.0),
            ],
        ),
        standard(
            "partially_offscreen",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                quad(-20.0, -10.0, 60.0, 40.0, "#dd0030", 0.8, 8.0, 2.0),
                quad(70.0, 50.0, 60.0, 40.0, "#20c090", 0.8, 8.0, 2.0),
                PaintCommand::PushOpacity(0.5),
                quad(80.0, -10.0, 40.0, 40.0, "#4060e0", 1.0, 0.0, 0.0),
                PaintCommand::PopOpacity,
            ],
        ),
        standard(
            "degenerate_quads",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                quad(10.0, 10.0, 0.0, 30.0, "#dd0030", 1.0, 0.0, 0.0),
                quad(20.0, 10.0, 30.0, 0.0, "#dd0030", 1.0, 0.0, 0.0),
                quad(30.0, 30.0, 0.5, 0.5, "#ece030", 1.0, 0.0, 0.0),
            ],
        ),
        standard("deep_group_nesting", Oklch::TRANSPARENT, deep_commands()),
        standard("instance_grid", Oklch::TRANSPARENT, grid_commands()),
        standard("clipped_nested_opacity", Oklch::TRANSPARENT, nested.clone()),
    ];
    nested.splice(
        3..3,
        [
            PaintCommand::PushOpacity(0.9),
            quad(2.0, 2.0, 30.0, 20.0, "#ff8000", 1.0, 5.0, 0.0),
            PaintCommand::PopOpacity,
        ],
    );
    cases.push(standard("sibling_groups_alias", Oklch::TRANSPARENT, nested));
    cases.extend(effect_cases());
    cases.extend(super::effects::cases());
    cases.extend(super::text::cases());
    cases.push(super::gpui::translucent_white_case());
    cases.extend(super::transforms::cases());
    cases.push(build(
        "extent_one_by_one",
        Extent {
            width: 1,
            height: 1,
        },
        Oklch::TRANSPARENT,
        &[quad(0.0, 0.0, 1.0, 1.0, "#dd0030", 0.5, 0.0, 0.0)],
    ));
    cases.push(build(
        "extent_odd_and_narrow",
        Extent {
            width: 37,
            height: 5,
        },
        hex("#102030", 1.0),
        &[
            PaintCommand::PushOpacity(0.6),
            quad(3.0, 0.5, 30.0, 4.0, "#ece030", 1.0, 1.5, 0.0),
            PaintCommand::PopOpacity,
        ],
    ));
    cases.push(build(
        "extent_wide",
        Extent {
            width: 257,
            height: 31,
        },
        Oklch::TRANSPARENT,
        &[
            quad(0.0, 0.0, 257.0, 31.0, "#253040", 1.0, 0.0, 0.0),
            quad(10.0, 5.0, 230.0, 20.0, "#4060e0", 0.8, 10.0, 2.0),
        ],
    ));
    cases
}

fn effect_cases() -> Vec<Case> {
    let mut blurred = stripes(STANDARD);
    blurred.push(backdrop_blur(rect(16.0, 12.0, 64.0, 48.0), 6.0, 10.0));
    let mut small_radius = stripes(STANDARD);
    small_radius.push(backdrop_blur(rect(10.0, 10.0, 70.0, 50.0), 2.5, 0.0));
    let mut large_radius = stripes(STANDARD);
    large_radius.push(backdrop_blur(rect(0.0, 0.0, 96.0, 72.0), 20.0, 0.0));
    let mut content = vec![backdrop()];
    content.extend([
        PaintCommand::PushContentBlur {
            bounds: rect(10.0, 10.0, 60.0, 50.0),
            radius: 4.0,
        },
        quad(14.0, 14.0, 30.0, 30.0, "#dd0030", 1.0, 0.0, 0.0),
        quad(30.0, 24.0, 30.0, 30.0, "#20c090", 0.8, 8.0, 0.0),
        PaintCommand::PopContentBlur,
        quad(60.0, 40.0, 30.0, 24.0, "#ece030", 1.0, 0.0, 0.0),
    ]);
    let mut blur_in_group = stripes(STANDARD);
    blur_in_group.extend([
        PaintCommand::PushOpacity(0.6),
        quad(4.0, 4.0, 80.0, 60.0, "#ffffff", 0.2, 0.0, 0.0),
        backdrop_blur(rect(10.0, 10.0, 60.0, 50.0), 5.0, 6.0),
        quad(20.0, 20.0, 30.0, 20.0, "#ffffff", 0.3, 4.0, 1.0),
        PaintCommand::PopOpacity,
    ]);
    let mut clipped_blur = stripes(STANDARD);
    clipped_blur.extend([
        clip(30.0, 20.0, 40.0, 30.0),
        backdrop_blur(rect(10.0, 10.0, 80.0, 50.0), 5.0, 0.0),
        PaintCommand::PopClip,
    ]);
    vec![
        standard(
            "gradient_horizontal",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                gradient(
                    8.0,
                    8.0,
                    80.0,
                    56.0,
                    ("#dd0030", 1.0),
                    ("#4060e0", 1.0),
                    (1.0, 0.0),
                    0.0,
                    0.0,
                ),
            ],
        ),
        standard(
            "gradient_diagonal_alpha_rounded_border",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                gradient(
                    8.0,
                    8.0,
                    80.0,
                    56.0,
                    ("#ece030", 1.0),
                    ("#a020e0", 0.2),
                    (1.0, 1.0),
                    12.0,
                    3.0,
                ),
            ],
        ),
        standard(
            "gradient_neutral_to_chroma",
            Oklch::TRANSPARENT,
            vec![gradient(
                0.0,
                0.0,
                96.0,
                72.0,
                ("#ffffff", 1.0),
                ("#ff0000", 1.0),
                (0.0, 1.0),
                0.0,
                0.0,
            )],
        ),
        standard(
            "shadow_soft_rounded",
            Oklch::TRANSPARENT,
            vec![
                quad(0.0, 0.0, 96.0, 72.0, "#e8e8e8", 1.0, 0.0, 0.0),
                shadow(
                    rect(26.0, 20.0, 44.0, 30.0),
                    8.0,
                    [0.0, 4.0],
                    6.0,
                    0.0,
                    "#000000",
                    0.5,
                ),
                quad(26.0, 20.0, 44.0, 30.0, "#ffffff", 1.0, 8.0, 0.0),
            ],
        ),
        standard(
            "shadow_offset_spread_negative",
            Oklch::TRANSPARENT,
            vec![
                quad(0.0, 0.0, 96.0, 72.0, "#e8e8e8", 1.0, 0.0, 0.0),
                shadow(
                    rect(20.0, 16.0, 50.0, 36.0),
                    6.0,
                    [6.0, 8.0],
                    3.0,
                    -4.0,
                    "#2040a0",
                    0.8,
                ),
                shadow(
                    rect(20.0, 16.0, 50.0, 36.0),
                    0.0,
                    [-4.0, -3.0],
                    2.0,
                    3.0,
                    "#a02040",
                    0.5,
                ),
            ],
        ),
        standard(
            "shadow_hard_edge",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                shadow(
                    rect(20.0, 16.0, 40.0, 30.0),
                    10.0,
                    [3.0, 3.0],
                    0.0,
                    0.0,
                    "#000000",
                    0.7,
                ),
            ],
        ),
        standard(
            "shadow_hard_edge_negative_spread",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                shadow(
                    rect(20.0, 16.0, 40.0, 30.0),
                    10.0,
                    [2.5, 1.5],
                    0.008,
                    -6.3,
                    "#000000",
                    0.7,
                ),
                shadow(
                    rect(8.0, 8.0, 10.0, 10.0),
                    4.0,
                    [60.0, 40.0],
                    0.04,
                    -7.0,
                    "#2040a0",
                    0.8,
                ),
            ],
        ),
        standard(
            "shadow_clipped_in_group",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                clip(10.0, 10.0, 50.0, 40.0),
                PaintCommand::PushOpacity(0.7),
                shadow(
                    rect(20.0, 16.0, 40.0, 30.0),
                    6.0,
                    [0.0, 2.0],
                    5.0,
                    1.0,
                    "#000000",
                    0.9,
                ),
                PaintCommand::PopOpacity,
                PaintCommand::PopClip,
            ],
        ),
        standard(
            "glyph_run_one_to_one",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                glyph_run(&[
                    (
                        rect(4.0, 4.0, 16.0, 16.0),
                        rect(0.0, 0.0, 16.0, 16.0),
                        "#ffffff",
                        1.0,
                    ),
                    (
                        rect(24.0, 4.0, 16.0, 16.0),
                        rect(16.0, 0.0, 16.0, 16.0),
                        "#ece030",
                        1.0,
                    ),
                    (
                        rect(44.0, 4.0, 16.0, 16.0),
                        rect(32.0, 0.0, 16.0, 16.0),
                        "#20c090",
                        0.6,
                    ),
                    (
                        rect(64.0, 4.0, 16.0, 16.0),
                        rect(48.0, 0.0, 16.0, 16.0),
                        "#dd0030",
                        1.0,
                    ),
                ]),
            ],
        ),
        standard(
            "glyph_run_clipped_in_group",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                clip(10.0, 20.0, 40.0, 30.0),
                PaintCommand::PushOpacity(0.5),
                glyph_run(&[
                    (
                        rect(4.0, 16.0, 16.0, 16.0),
                        rect(0.0, 16.0, 16.0, 16.0),
                        "#ffffff",
                        1.0,
                    ),
                    (
                        rect(24.0, 24.0, 16.0, 16.0),
                        rect(16.0, 16.0, 16.0, 16.0),
                        "#ffffff",
                        1.0,
                    ),
                    (
                        rect(40.0, 36.0, 16.0, 16.0),
                        rect(32.0, 16.0, 16.0, 16.0),
                        "#ffffff",
                        1.0,
                    ),
                ]),
                PaintCommand::PopOpacity,
                PaintCommand::PopClip,
            ],
        ),
        standard(
            "image_unscaled",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                photo(rect(8.0, 8.0, 32.0, 32.0), rect(0.0, 0.0, 32.0, 32.0), 0.0),
            ],
        ),
        standard(
            "image_scaled_rounded_subrect",
            Oklch::TRANSPARENT,
            vec![
                backdrop(),
                photo(
                    rect(10.25, 8.5, 60.0, 50.0),
                    rect(4.0, 4.0, 20.0, 24.0),
                    12.0,
                ),
                photo(
                    rect(70.0, 40.0, 20.0, 20.0),
                    rect(0.0, 0.0, 32.0, 32.0),
                    10.0,
                ),
            ],
        ),
        standard("backdrop_blur_rounded", Oklch::TRANSPARENT, blurred),
        standard(
            "backdrop_blur_small_radius",
            Oklch::TRANSPARENT,
            small_radius,
        ),
        standard(
            "backdrop_blur_large_radius",
            Oklch::TRANSPARENT,
            large_radius,
        ),
        standard("content_blur", Oklch::TRANSPARENT, content),
        standard(
            "backdrop_blur_inside_group",
            Oklch::TRANSPARENT,
            blur_in_group,
        ),
        standard("backdrop_blur_clipped", Oklch::TRANSPARENT, clipped_blur),
    ]
}
