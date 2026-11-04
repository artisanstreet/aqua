use aqua_core::{geometry::Rect, Oklch};
use aqua_graph::{
    Blend, ColorMatrix, CustomPassSpec, CustomShaderId, FramePlan, ImageInstance, PaintCommand,
    Pass, PassKind, PlanLimits, QuadInstance, RenderGraph, ShaderId, TextureFormat, TextureRef,
};
use aqua_protocol::Features;

use super::assets::standard_uploads;
use super::cases::{
    backdrop_blur, build, clip, glyph_run, gradient, hex, photo, quad, rect, shadow, stripes, Case,
    STANDARD,
};
use crate::kernels::{INVERT, ROW_BOX_BLUR, ROW_BOX_BLUR_TILE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    ColorMatrix,
    Compute,
    CustomPasses,
}

impl Requirement {
    pub fn offered_by(self, features: &Features) -> bool {
        match self {
            Requirement::ColorMatrix => features.color_matrix,
            Requirement::Compute => features.compute,
            Requirement::CustomPasses => features.custom_passes,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Requirement::ColorMatrix => "color matrix filters",
            Requirement::Compute => "compute passes",
            Requirement::CustomPasses => "custom passes",
        }
    }
}

pub fn requirements(plan: &FramePlan) -> Vec<Requirement> {
    let mut found = Vec::new();
    for pass in &plan.passes {
        let needed = match &pass.kind {
            PassKind::Filter {
                shader: ShaderId::ColorMatrix,
                ..
            } => Some(Requirement::ColorMatrix),
            PassKind::Compute { .. } => Some(Requirement::Compute),
            PassKind::Custom(_) => Some(Requirement::CustomPasses),
            _ => None,
        };
        if let Some(needed) = needed.filter(|needed| !found.contains(needed)) {
            found.push(needed);
        }
    }
    found
}

pub const INVERT_HLSL: &str = "Texture2D<float4> input0 : register(t0);\n\
cbuffer Parameters : register(b0) { float4 parameters[2]; };\n\
struct CustomInput { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
float4 invert(CustomInput input) : SV_Target {\n\
    float4 color = input0.Load(int3(input.position.xy, 0));\n\
    if (color.a <= 0.0) { return float4(0.0, 0.0, 0.0, 0.0); }\n\
    float3 straight = color.rgb / color.a;\n\
    float3 flipped = straight + (1.0 - 2.0 * straight) * saturate(parameters[0].x);\n\
    return float4(flipped * color.a, color.a);\n\
}\n";

fn invert_spec(amount: f32) -> CustomPassSpec {
    CustomPassSpec {
        name: INVERT.into(),
        hlsl: INVERT_HLSL.into(),
        entry: "invert".into(),
        bounds: STANDARD.bounds(),
        params: vec![amount],
    }
}

fn standard(name: &str, clear: Oklch, commands: Vec<PaintCommand>) -> Case {
    build(name, STANDARD, clear, &commands)
}

fn backdrop() -> PaintCommand {
    quad(0.0, 0.0, 96.0, 72.0, "#253040", 1.0, 0.0, 0.0)
}

fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32) -> PaintCommand {
    PaintCommand::PushRoundedClip {
        rect: rect(x, y, w, h),
        radius,
    }
}

fn clear_cases() -> Vec<Case> {
    vec![
        standard(
            "explicit_clear_opaque_all_work_culled",
            hex("#304050", 1.0),
            vec![
                quad(200.0, 200.0, 10.0, 10.0, "#dd0030", 1.0, 0.0, 0.0),
                PaintCommand::PushOpacity(0.0),
                quad(10.0, 10.0, 20.0, 20.0, "#dd0030", 1.0, 0.0, 0.0),
                PaintCommand::PopOpacity,
            ],
        ),
        standard(
            "explicit_clear_translucent_empty_frame",
            hex("#304050", 0.5),
            vec![],
        ),
        standard(
            "explicit_clear_before_backdrop_read",
            hex("#405060", 1.0),
            vec![backdrop_blur(rect(10.0, 10.0, 50.0, 40.0), 5.0, 6.0)],
        ),
    ]
}

fn rounded_clip_cases() -> Vec<Case> {
    let mut basic = stripes(STANDARD);
    basic.extend([
        rounded(10.0, 8.0, 60.0, 44.0, 14.0),
        quad(0.0, 0.0, 96.0, 72.0, "#ece030", 0.8, 0.0, 0.0),
        quad(20.0, 20.0, 60.0, 40.0, "#a020e0", 0.6, 6.0, 0.0),
        PaintCommand::PopClip,
    ]);
    let mut outer = stripes(STANDARD);
    outer.extend([
        clip(20.0, 10.0, 60.0, 50.0),
        rounded(14.5, 9.25, 50.5, 40.75, 9.5),
        gradient(
            10.0,
            5.0,
            70.0,
            55.0,
            ("#ece030", 1.0),
            ("#a020e0", 0.7),
            (1.0, 1.0),
            0.0,
            0.0,
        ),
        PaintCommand::PopClip,
        PaintCommand::PopClip,
    ]);
    let mut grouped = stripes(STANDARD);
    grouped.extend([
        PaintCommand::PushOpacity(0.6),
        rounded(8.0, 8.0, 70.0, 50.0, 12.0),
        quad(4.0, 4.0, 50.0, 40.0, "#dd0030", 0.9, 0.0, 0.0),
        quad(30.0, 20.0, 50.0, 40.0, "#4060e0", 0.9, 0.0, 0.0),
        shadow(
            rect(20.0, 16.0, 30.0, 20.0),
            4.0,
            [0.0, 3.0],
            4.0,
            0.0,
            "#000000",
            0.6,
        ),
        PaintCommand::PopClip,
        PaintCommand::PopOpacity,
    ]);
    let nested = vec![
        backdrop(),
        rounded(6.0, 6.0, 84.0, 60.0, 20.0),
        quad(0.0, 0.0, 96.0, 72.0, "#20c090", 1.0, 0.0, 0.0),
        rounded(24.0, 14.0, 60.0, 48.0, 16.0),
        quad(0.0, 0.0, 96.0, 72.0, "#dd0030", 0.8, 0.0, 0.0),
        PaintCommand::PopClip,
        PaintCommand::PopClip,
    ];
    let content = vec![
        backdrop(),
        rounded(8.0, 6.0, 70.0, 56.0, 10.0),
        glyph_run(&[
            (
                rect(12.0, 10.0, 16.0, 16.0),
                rect(0.0, 0.0, 16.0, 16.0),
                "#ffffff",
                1.0,
            ),
            (
                rect(64.0, 50.0, 16.0, 16.0),
                rect(16.0, 0.0, 16.0, 16.0),
                "#ece030",
                1.0,
            ),
        ]),
        photo(
            rect(20.0, 18.0, 40.0, 30.0),
            rect(0.0, 0.0, 32.0, 32.0),
            6.0,
        ),
        PaintCommand::PopClip,
    ];
    let oversized = vec![
        backdrop(),
        rounded(20.0, 20.0, 50.0, 30.0, 40.0),
        quad(0.0, 0.0, 96.0, 72.0, "#4060e0", 1.0, 0.0, 0.0),
        PaintCommand::PopClip,
    ];
    vec![
        standard("rounded_clip_basic", Oklch::TRANSPARENT, basic),
        standard(
            "rounded_clip_fractional_under_outer_clip",
            Oklch::TRANSPARENT,
            outer,
        ),
        standard(
            "rounded_clip_inside_opacity_group",
            Oklch::TRANSPARENT,
            grouped,
        ),
        standard("rounded_clip_nested", Oklch::TRANSPARENT, nested),
        standard("rounded_clip_glyphs_and_image", Oklch::TRANSPARENT, content),
        standard(
            "rounded_clip_radius_exceeds_half",
            Oklch::TRANSPARENT,
            oversized,
        ),
    ]
}

fn layer_cases() -> Vec<Case> {
    let far = vec![
        backdrop(),
        PaintCommand::PushOpacity(0.5),
        gradient(
            50.0,
            36.0,
            40.0,
            30.0,
            ("#dd0030", 1.0),
            ("#4060e0", 1.0),
            (1.0, 0.0),
            6.0,
            0.0,
        ),
        shadow(
            rect(56.0, 42.0, 24.0, 16.0),
            4.0,
            [0.0, 2.0],
            3.0,
            0.0,
            "#000000",
            0.7,
        ),
        quad(60.0, 40.0, 30.0, 26.0, "#ece030", 0.8, 0.0, 2.0),
        glyph_run(&[(
            rect(66.0, 44.0, 16.0, 16.0),
            rect(0.0, 0.0, 16.0, 16.0),
            "#ffffff",
            1.0,
        )]),
        PaintCommand::PopOpacity,
    ];
    let edge = vec![
        backdrop(),
        PaintCommand::PushOpacity(0.5),
        quad(70.0, 50.0, 40.0, 40.0, "#dd0030", 1.0, 8.0, 0.0),
        quad(80.0, 40.0, 40.0, 40.0, "#20c090", 1.0, 0.0, 0.0),
        PaintCommand::PopOpacity,
        PaintCommand::PushOpacity(0.7),
        quad(-20.0, -20.0, 40.0, 40.0, "#4060e0", 1.0, 0.0, 0.0),
        quad(-10.0, -10.0, 40.0, 40.0, "#ece030", 1.0, 5.0, 0.0),
        PaintCommand::PopOpacity,
    ];
    let nested = vec![
        backdrop(),
        PaintCommand::PushOpacity(0.8),
        quad(30.0, 20.0, 50.0, 40.0, "#4060e0", 0.9, 0.0, 0.0),
        PaintCommand::PushOpacity(0.5),
        quad(40.0, 30.0, 40.0, 30.0, "#dd0030", 1.0, 6.0, 0.0),
        quad(50.0, 36.0, 40.0, 30.0, "#ece030", 1.0, 0.0, 0.0),
        PaintCommand::PopOpacity,
        quad(34.0, 24.0, 20.0, 20.0, "#20c090", 0.7, 0.0, 0.0),
        PaintCommand::PopOpacity,
    ];
    vec![
        standard("bounded_layer_far_from_origin", Oklch::TRANSPARENT, far),
        standard(
            "bounded_layer_partially_offscreen",
            Oklch::TRANSPARENT,
            edge,
        ),
        standard(
            "bounded_layers_nested_off_origin",
            Oklch::TRANSPARENT,
            nested,
        ),
    ]
}

fn backdrop_cases() -> Vec<Case> {
    let translucent = vec![
        quad(0.0, 0.0, 96.0, 40.0, "#dd0030", 0.5, 0.0, 0.0),
        quad(20.0, 30.0, 60.0, 30.0, "#4060e0", 0.4, 0.0, 0.0),
        backdrop_blur(rect(14.0, 12.0, 66.0, 44.0), 6.0, 12.0),
    ];
    let mut grouped = stripes(STANDARD);
    grouped.extend([
        PaintCommand::PushOpacity(0.5),
        backdrop_blur(rect(12.0, 10.0, 64.0, 48.0), 7.0, 10.0),
        PaintCommand::PopOpacity,
    ]);
    let mut cases = vec![
        standard(
            "backdrop_replace_on_translucent_surface",
            hex("#102030", 0.4),
            translucent,
        ),
        standard(
            "backdrop_replace_with_group_opacity",
            Oklch::TRANSPARENT,
            grouped,
        ),
    ];
    for radius in [
        1.0f32, 1.7, 2.0, 6.0, 6.9, 7.5, 7.99, 8.01, 12.0, 14.5, 15.9, 17.0,
    ] {
        let mut commands = stripes(STANDARD);
        commands.push(backdrop_blur(rect(12.0, 10.0, 64.0, 48.0), radius, 8.0));
        cases.push(standard(
            &format!("backdrop_blur_radius_{radius}"),
            Oklch::TRANSPARENT,
            commands,
        ));
    }
    let mut content = vec![backdrop()];
    content.extend([
        PaintCommand::PushContentBlur {
            bounds: rect(10.0, 10.0, 60.0, 50.0),
            radius: 7.5,
        },
        quad(14.0, 14.0, 30.0, 30.0, "#dd0030", 1.0, 0.0, 0.0),
        quad(30.0, 24.0, 30.0, 30.0, "#20c090", 0.8, 8.0, 0.0),
        PaintCommand::PopContentBlur,
    ]);
    cases.push(standard(
        "content_blur_crossfade_radius",
        Oklch::TRANSPARENT,
        content,
    ));
    cases
}

fn color_matrix_cases() -> Vec<Case> {
    let mut gray = stripes(STANDARD);
    gray.extend([
        PaintCommand::PushColorMatrix {
            bounds: rect(10.0, 8.0, 70.0, 52.0),
            matrix: ColorMatrix::saturation(0.0),
        },
        quad(4.0, 4.0, 50.0, 40.0, "#dd0030", 0.8, 6.0, 0.0),
        quad(30.0, 24.0, 60.0, 40.0, "#20c090", 0.5, 0.0, 0.0),
        PaintCommand::PopColorMatrix,
    ]);
    let tint = ColorMatrix([
        0.5, 0.0, 0.0, 0.0, 0.4, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0,
        0.6, 0.1,
    ]);
    let offsets = vec![
        backdrop(),
        PaintCommand::PushColorMatrix {
            bounds: rect(12.5, 9.5, 60.0, 50.0),
            matrix: tint,
        },
        quad(20.0, 16.0, 40.0, 30.0, "#ece030", 0.7, 0.0, 0.0),
        PaintCommand::PopColorMatrix,
    ];
    let mut nested = stripes(STANDARD);
    nested.extend([
        PaintCommand::PushOpacity(0.7),
        rounded(8.0, 8.0, 76.0, 56.0, 14.0),
        PaintCommand::PushColorMatrix {
            bounds: rect(0.0, 0.0, 96.0, 72.0),
            matrix: ColorMatrix::saturation(2.0),
        },
        quad(10.0, 10.0, 50.0, 40.0, "#a020e0", 0.9, 0.0, 0.0),
        quad(40.0, 30.0, 40.0, 30.0, "#4060e0", 0.8, 0.0, 0.0),
        PaintCommand::PopColorMatrix,
        PaintCommand::PopClip,
        PaintCommand::PopOpacity,
    ]);
    vec![
        standard(
            "color_matrix_grayscale_over_stripes",
            Oklch::TRANSPARENT,
            gray,
        ),
        standard(
            "color_matrix_offsets_and_alpha",
            Oklch::TRANSPARENT,
            offsets,
        ),
        standard(
            "color_matrix_inside_rounded_clip_and_group",
            Oklch::TRANSPARENT,
            nested,
        ),
    ]
}

fn custom_cases() -> Vec<Case> {
    let effect = |amount: f32, bounds: Rect| {
        let mut commands = stripes(STANDARD);
        commands.extend([
            PaintCommand::PushCustomEffect {
                bounds,
                spec: invert_spec(amount),
            },
            quad(10.0, 8.0, 40.0, 30.0, "#dd0030", 0.9, 5.0, 0.0),
            quad(30.0, 24.0, 50.0, 30.0, "#20c090", 0.5, 0.0, 0.0),
            PaintCommand::PopCustomEffect,
        ]);
        commands
    };
    vec![
        standard(
            "custom_invert_full_amount",
            Oklch::TRANSPARENT,
            effect(1.0, rect(6.0, 6.0, 80.0, 56.0)),
        ),
        standard(
            "custom_invert_half_amount_fractional_bounds",
            Oklch::TRANSPARENT,
            effect(0.5, rect(6.5, 6.25, 70.5, 50.5)),
        ),
    ]
}

fn compute_case() -> Case {
    let mut graph = RenderGraph::new(STANDARD, hex("#101820", 1.0));
    let source = graph.create(STANDARD, TextureFormat::Rgba16Float);
    let blurred = graph.create(STANDARD, TextureFormat::Rgba16Float);
    let bar = |x: f32, color: &str, alpha: f32| QuadInstance {
        bounds: rect(x, 6.0, 8.0, 60.0),
        radius: 0.0,
        border_width: 0.0,
        fill: hex(color, alpha),
        border_color: Oklch::TRANSPARENT,
        clip: None,
        gradient: None,
        opacity: 1.0,
        linear: aqua_graph::Linear2::IDENTITY,
    };
    graph.push(Pass::new(
        PassKind::Instanced {
            shader: ShaderId::Quad,
            instances: vec![
                bar(10.0, "#dd0030", 1.0),
                bar(30.0, "#20c090", 0.8),
                bar(50.0, "#4060e0", 1.0),
                bar(71.5, "#ece030", 0.6),
            ],
        },
        Vec::new(),
        vec![source],
    ));
    graph.push(Pass::new(
        PassKind::Compute {
            shader: ShaderId::Custom(CustomShaderId(ROW_BOX_BLUR.0)),
            groups: [
                STANDARD.width.div_ceil(ROW_BOX_BLUR_TILE),
                STANDARD.height,
                1,
            ],
            params: [3.0, 0.0, 0.0, 0.0],
        },
        vec![source],
        vec![blurred],
    ));
    graph.push(Pass::new(
        PassKind::Images {
            instances: vec![ImageInstance {
                source: blurred,
                bounds: STANDARD.bounds(),
                texels: STANDARD.bounds(),
                radius: 0.0,
                opacity: 1.0,
                clip: None,
                clip_radius: 0.0,
                blend: Blend::Over,
                linear: aqua_graph::Linear2::IDENTITY,
            }],
        },
        vec![blurred],
        vec![TextureRef::Surface],
    ));
    let plan = graph
        .plan(PlanLimits::default())
        .unwrap_or_else(|error| panic!("compute case does not plan: {error}"));
    Case {
        name: "compute_row_box_blur".into(),
        plan,
        uploads: standard_uploads(),
    }
}

pub fn cases() -> Vec<Case> {
    let mut cases = clear_cases();
    cases.extend(rounded_clip_cases());
    cases.extend(layer_cases());
    cases.extend(backdrop_cases());
    cases.extend(color_matrix_cases());
    cases.extend(custom_cases());
    cases.push(compute_case());
    cases
}
