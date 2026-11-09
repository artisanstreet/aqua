#[path = "../../src/scene/geometry.rs"]
mod implementation;

use aqua_core::{Oklch, Rect};
use aqua_graph::{plan_paint, CustomPassSpec, Extent, PaintCommand, PassKind, Quad};
use implementation::GeometryCache;

fn plan() -> aqua_graph::FramePlan {
    plan_paint(
        Extent::new(64, 64),
        Oklch::BLACK,
        &[PaintCommand::Quad(Quad {
            bounds: Rect {
                x: 1.0,
                y: 2.0,
                w: 12.0,
                h: 16.0,
            },
            radius: 4.0,
            border_width: 1.0,
            fill: Oklch::new(0.6, 0.1, 40.0),
            border_color: Oklch::BLACK,
        })],
    )
    .unwrap()
}

#[test]
fn identical_geometry_reuses_packing_but_edits_and_removals_do_not() {
    let mut cache = GeometryCache::default();
    let original = plan();
    assert!(!cache.prepare(&original));
    assert!(cache.prepare(&original));
    let mut edited = original.clone();
    let PassKind::Instanced { instances, .. } = &mut edited.passes[0].kind else {
        unreachable!()
    };
    instances[0].bounds.x += 1.0;
    assert!(!cache.prepare(&edited));
    assert!(cache.prepare(&edited));
    assert!(!cache.prepare(&original));
    edited.passes.clear();
    assert!(!cache.prepare(&edited));
    assert!(cache.prepare(&edited));
    assert!(!cache.prepare(&original));
}

#[test]
fn shader_uniform_changes_keep_geometry_while_the_plan_still_updates() {
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: 24.0,
        h: 24.0,
    };
    let mut shader = plan_paint(
        Extent::new(64, 64),
        Oklch::BLACK,
        &[
            PaintCommand::PushCustomEffect {
                bounds,
                spec: CustomPassSpec {
                    isolate_backdrop: false,
                    reference: None,
                    name: "geometry.test".into(),
                    hlsl: String::new(),
                    entry: "main".into(),
                    bounds,
                    params: vec![0.0; 4],
                },
            },
            PaintCommand::PopCustomEffect,
        ],
    )
    .unwrap();
    let mut cache = GeometryCache::default();
    assert!(!cache.prepare(&shader));
    for time in [0.01, 0.5, 100.0] {
        for pass in &mut shader.passes {
            if let PassKind::Custom(spec) = &mut pass.kind {
                spec.params[0] = time;
            }
        }
        assert!(cache.prepare(&shader));
    }
}

#[test]
fn color_opacity_clip_and_scale_changes_invalidate_packed_instances() {
    let original = plan();
    for field in 0..4 {
        let mut cache = GeometryCache::default();
        assert!(!cache.prepare(&original));
        let mut changed = original.clone();
        let PassKind::Instanced { instances, .. } = &mut changed.passes[0].kind else {
            unreachable!()
        };
        match field {
            0 => instances[0].fill = Oklch::new(0.9, 0.0, 0.0),
            1 => instances[0].opacity = 0.5,
            2 => {
                instances[0].clip = Some(Rect {
                    x: 2.0,
                    y: 2.0,
                    w: 3.0,
                    h: 4.0,
                })
            }
            _ => instances[0].bounds.w *= 2.0,
        }
        assert!(!cache.prepare(&changed));
        assert!(cache.prepare(&changed));
    }
}

#[test]
fn oversized_geometry_is_not_retained_and_does_not_poison_the_next_scene() {
    let mut cache = GeometryCache::default();
    let original = plan();
    assert!(!cache.prepare(&original));
    let mut oversized = original.clone();
    let PassKind::Instanced { instances, .. } = &mut oversized.passes[0].kind else {
        unreachable!()
    };
    instances.resize(100_000, instances[0]);
    assert!(!cache.prepare(&oversized));
    assert!(!cache.prepare(&oversized));
    assert!(!cache.prepare(&original));
    assert!(cache.prepare(&original));
}

#[cfg(windows)]
#[test]
fn retained_gpu_geometry_matches_fresh_pixels_after_edits_trim_and_recovery() {
    use aqua_gpu_d3d11::hal::budget::Trim;
    use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend};
    use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, SurfaceTarget};

    for preference in [AdapterPreference::Warp, AdapterPreference::HighPerformance] {
        let mut backend = OffscreenBackend::new(preference).unwrap();
        let mut plan = plan();
        let surface = backend
            .create_surface(SurfaceTarget::Offscreen(plan.extent))
            .unwrap();
        let inputs = FrameInputs::new(surface);
        backend.execute(&plan, &inputs).unwrap();
        let original = backend.read_surface(surface).unwrap();
        assert!(backend.last_scene_stats().upload_bytes > 0);
        backend.execute(&plan, &inputs).unwrap();
        assert_eq!(backend.last_scene_stats().upload_bytes, 0);
        assert_eq!(backend.read_surface(surface).unwrap(), original);
        let PassKind::Instanced { instances, .. } = &mut plan.passes[0].kind else {
            unreachable!()
        };
        instances[0].fill = Oklch::new(0.8, 0.15, 230.0);
        backend.execute(&plan, &inputs).unwrap();
        assert!(backend.last_scene_stats().upload_bytes > 0);
        let edited = backend.read_surface(surface).unwrap();
        assert_ne!(edited, original);
        backend
            .trim(&Trim {
                shrink_upload_ring: true,
                free_transients: true,
                evict_idle_resources: true,
            })
            .unwrap();
        backend.execute(&plan, &inputs).unwrap();
        assert!(backend.last_scene_stats().upload_bytes > 0);
        assert_eq!(backend.read_surface(surface).unwrap(), edited);
        backend.recover().unwrap();
        backend.execute(&plan, &inputs).unwrap();
        assert!(backend.last_scene_stats().upload_bytes > 0);
        assert_eq!(backend.read_surface(surface).unwrap(), edited);
    }
}

#[cfg(windows)]
#[test]
fn shader_uniforms_advance_without_reuploading_retained_geometry() {
    use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend};
    use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, SurfaceTarget};

    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: 24.0,
        h: 24.0,
    };
    let mut plan = plan_paint(Extent::new(64, 64), Oklch::BLACK, &[
        PaintCommand::PushCustomEffect { bounds, spec: CustomPassSpec {
            isolate_backdrop: false,
            reference: None,
            name: "retained.uniforms".into(),
            hlsl: "cbuffer P : register(b0) { float4 p; }; float4 main() : SV_Target { return float4(p.x, 0.0, 1.0-p.x, 1.0); }".into(),
            entry: "main".into(),
            bounds,
            params: vec![0.0; 4],
        } },
        PaintCommand::PopCustomEffect,
    ]).unwrap();
    let mut backend = OffscreenBackend::new(AdapterPreference::HighPerformance).unwrap();
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(plan.extent))
        .unwrap();
    let inputs = FrameInputs::new(surface);
    backend.execute(&plan, &inputs).unwrap();
    let before = backend.read_surface(surface).unwrap();
    for pass in &mut plan.passes {
        if let PassKind::Custom(spec) = &mut pass.kind {
            spec.params[0] = 1.0;
        }
    }
    backend.execute(&plan, &inputs).unwrap();
    assert_eq!(backend.last_scene_stats().upload_bytes, 0);
    let after = backend.read_surface(surface).unwrap();
    let at = (8 * 64 + 8) * 4;
    assert!(before.rgba8[at + 2] > before.rgba8[at]);
    assert!(after.rgba8[at] > after.rgba8[at + 2]);
}
