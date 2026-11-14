#![cfg(windows)]

mod support;

use aqua_core::Oklch;
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::conformance::cases::{build, quad, STANDARD};
use aqua_gpu_ref::conformance::Session;
use aqua_graph::{
    Blend, CustomPassSpec, CustomShaderId, ImageInstance, PaintCommand, Pass, PassKind, PlanLimits,
    RenderGraph, ShaderId, TextureFormat, TextureRef,
};
use aqua_protocol::SurfacePixels;

fn backend() -> OffscreenBackend {
    OffscreenBackend::with_format(AdapterPreference::Warp, SurfaceFormat::Rgba16Float)
        .expect("WARP must be available")
}

fn at(pixels: &SurfacePixels, x: usize, y: usize) -> [u8; 4] {
    let index = (y * pixels.extent.width as usize + x) * 4;
    pixels.rgba8[index..index + 4].try_into().unwrap()
}

fn parameter_probe(params: Vec<f32>, hlsl: &str) -> Vec<PaintCommand> {
    vec![
        PaintCommand::PushCustomEffect {
            bounds: STANDARD.bounds(),
            spec: CustomPassSpec {
                isolate_backdrop: false,
                reference: None,
                name: "probe".into(),
                hlsl: hlsl.into(),
                entry: "entry".into(),
                bounds: STANDARD.bounds(),
                params,
            },
        },
        quad(0.0, 0.0, 96.0, 72.0, "#ffffff", 1.0, 0.0, 0.0),
        PaintCommand::PopCustomEffect,
    ]
}

const PROBE_HLSL: &str = "cbuffer Parameters : register(b0) { float4 parameters[5]; };\n\
struct CustomInput { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
float4 entry(CustomInput input) : SV_Target { return float4(parameters[4].x, parameters[4].y, parameters[1].w, 1.0); }\n";

#[test]
fn custom_passes_read_every_packed_parameter() {
    let _software = support::software();
    let mut params = vec![0.0f32; 20];
    params[16] = 1.0;
    params[17] = 0.5;
    params[7] = 0.0;
    let case = build(
        "probe",
        STANDARD,
        Oklch::TRANSPARENT,
        &parameter_probe(params, PROBE_HLSL),
    );
    let mut backend = backend();
    let pixels = Session::new(&mut backend).render_case(&case).unwrap();
    let [r, g, b, a] = at(&pixels, 40, 30);
    assert_eq!((r, b, a), (255, 0, 255));
    assert!(g.abs_diff(128) <= 1, "{g}");
}

#[test]
fn invalid_custom_hlsl_is_reported_without_poisoning_the_backend() {
    let _software = support::software();
    let mut backend = backend();
    let broken = build(
        "broken",
        STANDARD,
        Oklch::TRANSPARENT,
        &parameter_probe(vec![0.0; 4], "float4 entry( : SV_Target { return 1; }"),
    );
    let error = Session::new(&mut backend).render_case(&broken).unwrap_err();
    assert!(error.to_string().contains("shader compilation"), "{error}");
    let healthy = build(
        "healthy",
        STANDARD,
        Oklch::TRANSPARENT,
        &parameter_probe(vec![0.0; 20], PROBE_HLSL),
    );
    Session::new(&mut backend).render_case(&healthy).unwrap();
}

const FILL_HLSL: &str = "RWTexture2D<float4> output : register(u0);\n\
cbuffer Parameters : register(b0) { float4 parameters; };\n\
[numthreads(8, 8, 1)] void fill(uint3 id : SV_DispatchThreadID) { output[id.xy] = parameters; }\n";

#[test]
fn registered_compute_shaders_write_their_output() {
    let _software = support::software();
    let mut backend = backend();
    let id = CustomShaderId(7);
    backend.register_compute(id, FILL_HLSL, "fill").unwrap();
    let mut graph = RenderGraph::new(STANDARD, Oklch::TRANSPARENT);
    let target = graph.create(STANDARD, TextureFormat::Rgba16Float);
    graph.push(Pass::new(
        PassKind::Compute {
            shader: ShaderId::Custom(id),
            groups: [STANDARD.width.div_ceil(8), STANDARD.height.div_ceil(8), 1],
            params: [0.0, 1.0, 0.0, 1.0],
        },
        Vec::new(),
        vec![target],
    ));
    graph.push(Pass::new(
        PassKind::Images {
            instances: vec![ImageInstance {
                source: target,
                bounds: STANDARD.bounds(),
                texels: STANDARD.bounds(),
                radius: 0.0,
                opacity: 1.0,
                clip: None,
                clip_radius: 0.0,
                blend: Blend::Over,
                linear: aqua_graph::Linear2::IDENTITY,
                mask: None,
            }],
        },
        vec![target],
        vec![TextureRef::Surface],
    ));
    let plan = graph.plan(PlanLimits::default()).unwrap();
    let pixels = Session::new(&mut backend).render(&plan).unwrap();
    assert_eq!(at(&pixels, 95, 71), [0, 255, 0, 255]);
    assert_eq!(at(&pixels, 0, 0), [0, 255, 0, 255]);
}
