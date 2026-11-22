#![cfg(target_os = "macos")]

mod support;

#[path = "fixtures/editor/solar_horizon.rs"]
mod solar_horizon;
#[path = "fixtures/editor/winter_sky.rs"]
mod winter_sky;

use aqua_core::geometry::Rect;
use aqua_core::shader::ReferenceShader;
use aqua_core::Oklch;
use aqua_gpu_metal::scene::dxc::LoadError as DxcLoadError;
use aqua_gpu_metal::scene::spirv_cross::LoadError as SpirvCrossLoadError;
use aqua_gpu_metal::scene::{
    Attempt, LibrarySource, LoadError, OffscreenBackend, OffscreenOptions, ShaderCompiler,
    SurfaceFormat,
};
use aqua_gpu_ref::conformance::cases::{build, quad, Case, STANDARD};
use aqua_gpu_ref::conformance::{run_case, Session, Tolerance};
use aqua_gpu_ref::RefBackend;
use aqua_graph::{
    Blend, CustomPassSpec, CustomShaderId, Extent, ImageInstance, PaintCommand, Pass, PassKind,
    PlanLimits, RenderGraph, ShaderId, TextureFormat, TextureRef,
};
use aqua_protocol::{GpuBackend, GpuError, SurfacePixels};

const EDITOR: Extent = Extent {
    width: 320,
    height: 200,
};

fn backend() -> Option<OffscreenBackend> {
    support::open(SurfaceFormat::Rgba16Float).and_then(support::compiling)
}

fn at(pixels: &SurfacePixels, x: usize, y: usize) -> [u8; 4] {
    let index = (y * pixels.extent.width as usize + x) * 4;
    pixels.rgba8[index..index + 4].try_into().unwrap()
}

fn effect(spec: CustomPassSpec) -> Vec<PaintCommand> {
    vec![
        PaintCommand::PushCustomEffect {
            bounds: spec.bounds,
            spec,
        },
        PaintCommand::PopCustomEffect,
    ]
}

fn spec(name: &str, hlsl: &str, entry: &str, bounds: Rect, params: Vec<f32>) -> CustomPassSpec {
    CustomPassSpec {
        isolate_backdrop: false,
        reference: None,
        name: name.into(),
        hlsl: hlsl.into(),
        entry: entry.into(),
        bounds,
        params,
    }
}

fn artwork() -> Rect {
    Rect {
        x: 2.0,
        y: 2.0,
        w: EDITOR.width as f32 - 4.0,
        h: EDITOR.height as f32 - 4.0,
    }
}

fn frame_uniforms(bounds: Rect, time: f32) -> Vec<f32> {
    vec![bounds.w, bounds.h, time, 1.0]
}

fn winter_sky(time: f32) -> CustomPassSpec {
    let bounds = artwork();
    let mut params = frame_uniforms(bounds, time);
    params.extend([17.0 / 255.0, 17.0 / 255.0, 17.0 / 255.0, 1.0]);
    params.extend([
        0.163_456_72,
        0.001_828_275,
        -0.052_028_373,
        1.0,
        0.243_426_04,
        -0.000_731_804,
        -0.089_916_567,
        1.0,
        0.366_245_7,
        -0.011_359_377,
        -0.113_559,
        1.0,
        0.571_379_2,
        -0.026_343_03,
        -0.093_552_39,
        1.0,
        0.805_291_95,
        -0.018_475_078,
        -0.042_172_065,
        1.0,
    ]);
    params.extend([0.94, 0.97, 0.99, 16.4]);
    params.extend([0.4, 0.0, 0.0, 0.0]);
    CustomPassSpec {
        reference: Some(ReferenceShader(winter_sky::shade)),
        ..spec(
            "artisan.winter-sky",
            include_str!("fixtures/editor/winter_sky.hlsl"),
            "winter_sky",
            bounds,
            params,
        )
    }
}

fn solar_horizon(time: f32) -> CustomPassSpec {
    let bounds = artwork();
    let mut params = frame_uniforms(bounds, time);
    params.extend([0.0056, 0.0056, 0.0056, 1.0]);
    params.extend([0.13, 0.25, 0.45, 1.0]);
    params.extend([0.75, 0.32, 0.08, 1.0]);
    params.extend([0.88, 0.80, 0.55, 1.0]);
    params.extend([0.42, 16.4, 0.0, 0.0]);
    params.extend([0.60, 0.10, 0.06, 1.0]);
    CustomPassSpec {
        reference: Some(ReferenceShader(solar_horizon::shade)),
        ..spec(
            "artisan.solar-horizon",
            include_str!("fixtures/editor/solar_horizon.hlsl"),
            "solar_horizon",
            bounds,
            params,
        )
    }
}

fn assert_matches_reference(backend: &mut OffscreenBackend, case: &Case) {
    let comparison = run_case(&mut RefBackend::new(), backend, case, Tolerance::default())
        .unwrap_or_else(|error| panic!("{}: {error}", case.name));
    eprintln!(
        "{}: interior {} soft {} edge {} ({} interior pixels)",
        case.name,
        comparison.interior_max,
        comparison.soft_max,
        comparison.edge_max,
        comparison.interior_pixels
    );
    assert!(
        comparison.passed(),
        "{} differs from its reference: {:?}",
        case.name,
        comparison.failures
    );
}

#[test]
fn the_editor_winter_sky_renders_like_its_reference_shader() {
    let Some(mut backend) = backend() else {
        return;
    };
    for time in [0.0, 37.25] {
        let case = build(
            format!("winter_sky_at_{time}s"),
            EDITOR,
            Oklch::TRANSPARENT,
            &effect(winter_sky(time)),
        );
        assert_matches_reference(&mut backend, &case);
    }
    support::assert_clean(&backend);
}

#[test]
fn the_retired_solar_horizon_renders_like_its_reference_shader() {
    let Some(mut backend) = backend() else {
        return;
    };
    for time in [0.0, 12.5] {
        let case = build(
            format!("solar_horizon_at_{time}s"),
            EDITOR,
            Oklch::TRANSPARENT,
            &effect(solar_horizon(time)),
        );
        assert_matches_reference(&mut backend, &case);
    }
    support::assert_clean(&backend);
}

#[test]
fn the_editor_sky_renders_the_same_on_an_eight_bit_surface() {
    let Some(mut backend) = support::open(SurfaceFormat::Bgra8Unorm).and_then(support::compiling)
    else {
        return;
    };
    let case = build(
        "winter_sky_eight_bit",
        EDITOR,
        Oklch::TRANSPARENT,
        &effect(winter_sky(4.0)),
    );
    assert_matches_reference(&mut backend, &case);
    support::assert_clean(&backend);
}

const PROBE_HLSL: &str = "cbuffer Parameters : register(b0) { float4 parameters[5]; };\n\
struct CustomInput { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
float4 entry(CustomInput input) : SV_Target { return float4(parameters[4].x, parameters[4].y, parameters[1].w, 1.0); }\n";

fn parameter_probe(params: Vec<f32>, hlsl: &str) -> Vec<PaintCommand> {
    vec![
        PaintCommand::PushCustomEffect {
            bounds: STANDARD.bounds(),
            spec: spec("probe", hlsl, "entry", STANDARD.bounds(), params),
        },
        quad(0.0, 0.0, 96.0, 72.0, "#ffffff", 1.0, 0.0, 0.0),
        PaintCommand::PopCustomEffect,
    ]
}

#[test]
fn custom_passes_read_every_packed_parameter() {
    let Some(mut backend) = backend() else {
        return;
    };
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
    let pixels = Session::new(&mut backend).render_case(&case).unwrap();
    let [r, g, b, a] = at(&pixels, 40, 30);
    assert_eq!((r, b, a), (255, 0, 255));
    assert!(g.abs_diff(128) <= 1, "{g}");
    support::assert_clean(&backend);
}

#[test]
fn invalid_custom_hlsl_is_reported_without_poisoning_the_backend() {
    let Some(mut backend) = backend() else {
        return;
    };
    let broken = build(
        "broken",
        STANDARD,
        Oklch::TRANSPARENT,
        &parameter_probe(vec![0.0; 4], "float4 entry( : SV_Target { return 1; }"),
    );
    let error = Session::new(&mut backend)
        .render_case(&broken)
        .unwrap_err()
        .to_string();
    assert!(error.contains("shader compilation"), "{error}");
    assert!(
        error.contains("aqua-custom"),
        "the diagnostic names the source: {error}"
    );
    let healthy = build(
        "healthy",
        STANDARD,
        Oklch::TRANSPARENT,
        &parameter_probe(vec![0.0; 20], PROBE_HLSL),
    );
    Session::new(&mut backend).render_case(&healthy).unwrap();
    support::assert_clean(&backend);
}

const UNBOUND_HLSL: &str = "Texture2D<float4> input3 : register(t3);\n\
struct CustomInput { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
float4 entry(CustomInput input) : SV_Target { return input3.Load(int3(0, 0, 0)) + float4(0.0, 0.0, 0.0, 1.0); }\n";

#[test]
fn custom_inputs_without_a_texture_read_as_transparent_black() {
    let Some(mut backend) = backend() else {
        return;
    };
    let case = build(
        "unbound",
        STANDARD,
        Oklch::TRANSPARENT,
        &effect(spec(
            "unbound",
            UNBOUND_HLSL,
            "entry",
            STANDARD.bounds(),
            Vec::new(),
        )),
    );
    let pixels = Session::new(&mut backend).render_case(&case).unwrap();
    assert_eq!(at(&pixels, 40, 30), [0, 0, 0, 255]);
    support::assert_clean(&backend);
}

const FILL_HLSL: &str = "RWTexture2D<float4> output : register(u0);\n\
cbuffer Parameters : register(b0) { float4 parameters; };\n\
[numthreads(8, 8, 1)] void fill(uint3 id : SV_DispatchThreadID) { output[id.xy] = parameters; }\n";

fn fill_plan(id: CustomShaderId) -> aqua_graph::FramePlan {
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
    graph.plan(PlanLimits::default()).unwrap()
}

#[test]
fn registered_compute_shaders_write_their_output() {
    let Some(mut backend) = backend() else {
        return;
    };
    let id = CustomShaderId(7);
    let unregistered = Session::new(&mut backend)
        .render(&fill_plan(id))
        .unwrap_err()
        .to_string();
    assert!(
        unregistered.contains("unregistered compute shader"),
        "{unregistered}"
    );
    let rejected = backend
        .register_compute(id, "[numthreads(8, 8, 1)] void fill( {", "fill")
        .unwrap_err()
        .to_string();
    assert!(rejected.contains("shader compilation"), "{rejected}");
    backend.register_compute(id, FILL_HLSL, "fill").unwrap();
    let pixels = Session::new(&mut backend).render(&fill_plan(id)).unwrap();
    assert_eq!(at(&pixels, 95, 71), [0, 255, 0, 255]);
    assert_eq!(at(&pixels, 0, 0), [0, 255, 0, 255]);
    support::assert_clean(&backend);
}

fn assert_tried_only(attempts: &[Attempt], missing: &std::path::Path) {
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    assert_eq!(attempts[0].location, missing.as_os_str());
}

fn assert_unsupported_without_a_library(compiler: ShaderCompiler) -> Option<OffscreenBackend> {
    let options = OffscreenOptions {
        compiler,
        ..OffscreenOptions::default()
    };
    let mut backend = support::open_with(options)?;
    assert!(!backend.capabilities().features.custom_passes);
    let case = build(
        "sky_without_compiler",
        EDITOR,
        Oklch::TRANSPARENT,
        &effect(winter_sky(0.0)),
    );
    match Session::new(&mut backend).render_case(&case) {
        Err(aqua_gpu_ref::conformance::RunError::Backend(GpuError::Unsupported(what))) => {
            assert_eq!(what, "custom passes")
        }
        other => panic!("custom passes are unsupported without a compiler, got {other:?}"),
    }
    assert!(matches!(
        backend.register_compute(CustomShaderId(7), FILL_HLSL, "fill"),
        Err(GpuError::Unsupported(_))
    ));
    let healthy = build(
        "quads_without_compiler",
        STANDARD,
        Oklch::TRANSPARENT,
        &[quad(4.0, 4.0, 40.0, 30.0, "#20c090", 1.0, 4.0, 0.0)],
    );
    Session::new(&mut backend).render_case(&healthy).unwrap();
    support::assert_clean(&backend);
    Some(backend)
}

#[test]
fn without_libdxcompiler_custom_passes_are_reported_unsupported() {
    let missing = std::env::temp_dir().join("aqua-no-such-dir/libdxcompiler.dylib");
    let Some(backend) = assert_unsupported_without_a_library(ShaderCompiler::Enabled {
        dxcompiler: LibrarySource::Library(missing.clone()),
        spirv_cross: LibrarySource::from_env("AQUA_SPIRV_CROSS_C"),
    }) else {
        return;
    };
    match backend.runtime_compiler().error() {
        Some(LoadError::Dxc(DxcLoadError::NotFound { attempts })) => {
            assert_tried_only(attempts, &missing)
        }
        Some(LoadError::Both { dxc, .. }) => match dxc.as_ref() {
            DxcLoadError::NotFound { attempts } => assert_tried_only(attempts, &missing),
            other => panic!("a missing libdxcompiler is reported as not found, got {other:?}"),
        },
        other => panic!("a missing libdxcompiler is reported as not found, got {other:?}"),
    }
}

#[test]
fn without_spirv_cross_custom_passes_are_reported_unsupported() {
    let missing = std::env::temp_dir().join("aqua-no-such-dir/libspirv-cross-c-shared.dylib");
    let Some(backend) = assert_unsupported_without_a_library(ShaderCompiler::Enabled {
        dxcompiler: LibrarySource::from_env("AQUA_DXCOMPILER"),
        spirv_cross: LibrarySource::Library(missing.clone()),
    }) else {
        return;
    };
    match backend.runtime_compiler().error() {
        Some(LoadError::SpirvCross(SpirvCrossLoadError::NotFound { attempts })) => {
            assert_tried_only(attempts, &missing)
        }
        Some(LoadError::Both { spirv_cross, .. }) => match spirv_cross.as_ref() {
            SpirvCrossLoadError::NotFound { attempts } => assert_tried_only(attempts, &missing),
            other => panic!("a missing SPIRV-Cross is reported as not found, got {other:?}"),
        },
        other => panic!("a missing SPIRV-Cross is reported as not found, got {other:?}"),
    }
}
