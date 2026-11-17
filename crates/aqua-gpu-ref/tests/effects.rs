use aqua_core::Oklch;
use aqua_gpu_ref::conformance::cases::{backdrop_blur, build, hex, quad, rect};
use aqua_gpu_ref::conformance::{requirements, Requirement, Session};
use aqua_gpu_ref::kernels::INVERT;
use aqua_gpu_ref::RefBackend;
use aqua_graph::{ColorMatrix, CustomPassSpec, Extent, PaintCommand, PassKind, ShaderId};
use aqua_protocol::{GpuBackend, SurfacePixels};

const EXTENT: Extent = Extent::new(48, 32);

fn render(backend: &mut RefBackend, clear: Oklch, commands: &[PaintCommand]) -> SurfacePixels {
    let case = build("effect", EXTENT, clear, commands);
    Session::new(backend).render_case(&case).expect("renders")
}

fn at(pixels: &SurfacePixels, x: usize, y: usize) -> [u8; 4] {
    let index = (y * pixels.extent.width as usize + x) * 4;
    pixels.rgba8[index..index + 4].try_into().unwrap()
}

#[test]
fn explicit_clear_yields_the_clear_colour_when_nothing_draws() {
    let mut backend = RefBackend::new();
    let clear = hex("#406080", 0.5);
    let pixels = render(&mut backend, clear, &[]);
    let expected = clear
        .to_gpu()
        .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    assert_eq!(at(&pixels, 0, 0), expected);
    assert_eq!(at(&pixels, 47, 31), expected);
}

#[test]
fn rounded_clip_removes_the_corner_and_keeps_the_middle() {
    let mut backend = RefBackend::new();
    let pixels = render(
        &mut backend,
        Oklch::TRANSPARENT,
        &[
            PaintCommand::PushRoundedClip {
                rect: rect(4.0, 4.0, 40.0, 24.0),
                radius: 10.0,
            },
            quad(0.0, 0.0, 48.0, 32.0, "#ffffff", 1.0, 0.0, 0.0),
            PaintCommand::PopClip,
        ],
    );
    assert_eq!(at(&pixels, 4, 4)[3], 0);
    assert_eq!(at(&pixels, 24, 16)[3], 255);
    assert_eq!(at(&pixels, 24, 3)[3], 0);
    assert!(at(&pixels, 24, 4)[3] > 250);
    let corner = at(&pixels, 6, 7)[3];
    assert!(corner > 0 && corner < 255, "{corner}");
}

#[test]
fn backdrop_blur_replaces_translucent_backdrop_rather_than_stacking() {
    let mut backend = RefBackend::new();
    let pixels = render(
        &mut backend,
        Oklch::TRANSPARENT,
        &[
            quad(0.0, 0.0, 48.0, 32.0, "#ffffff", 0.5, 0.0, 0.0),
            backdrop_blur(rect(8.0, 8.0, 32.0, 16.0), 3.0, 0.0),
        ],
    );
    let inside = at(&pixels, 24, 16);
    let outside = at(&pixels, 2, 2);
    assert_eq!(inside, outside);
    assert!((inside[3] as i32 - 128).abs() <= 1);
}

#[test]
fn color_matrix_grayscale_equalizes_channels() {
    let mut backend = RefBackend::new();
    let pixels = render(
        &mut backend,
        Oklch::TRANSPARENT,
        &[
            PaintCommand::PushColorMatrix {
                bounds: rect(0.0, 0.0, 48.0, 32.0),
                matrix: ColorMatrix::saturation(0.0),
            },
            quad(0.0, 0.0, 48.0, 32.0, "#dd0030", 1.0, 0.0, 0.0),
            PaintCommand::PopColorMatrix,
        ],
    );
    let [r, g, b, a] = at(&pixels, 24, 16);
    assert_eq!(a, 255);
    assert!(r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1, "{r} {g} {b}");
}

#[test]
fn custom_passes_run_registered_kernels_and_refuse_unknown_ones() {
    let spec = |name: &str| CustomPassSpec {
        isolate_backdrop: false,
        reference: None,
        name: name.into(),
        hlsl: String::new(),
        entry: "main".into(),
        bounds: EXTENT.bounds(),
        params: vec![1.0],
    };
    let scene = |name: &str| {
        vec![
            PaintCommand::PushCustomEffect {
                bounds: EXTENT.bounds(),
                spec: spec(name),
            },
            quad(0.0, 0.0, 48.0, 32.0, "#000000", 1.0, 0.0, 0.0),
            PaintCommand::PopCustomEffect,
        ]
    };
    let mut backend = RefBackend::new();
    let pixels = render(&mut backend, Oklch::TRANSPARENT, &scene(INVERT));
    assert_eq!(at(&pixels, 10, 10), [255, 255, 255, 255]);

    let case = build(
        "unknown",
        EXTENT,
        Oklch::TRANSPARENT,
        &scene("no.such.kernel"),
    );
    let error = Session::new(&mut backend).render_case(&case).unwrap_err();
    assert!(error.to_string().contains("unsupported"), "{error}");
    assert!(matches!(
        backend.capabilities().features,
        features if features.custom_passes && features.compute && features.color_matrix
    ));
}

#[test]
fn plans_report_the_features_they_need() {
    let plain = build(
        "plain",
        EXTENT,
        Oklch::TRANSPARENT,
        &[quad(0.0, 0.0, 8.0, 8.0, "#fff", 1.0, 0.0, 0.0)],
    );
    assert!(requirements(&plain.plan).is_empty());
    let filtered = build(
        "filtered",
        EXTENT,
        Oklch::TRANSPARENT,
        &[
            PaintCommand::PushColorMatrix {
                bounds: EXTENT.bounds(),
                matrix: ColorMatrix::IDENTITY,
            },
            quad(0.0, 0.0, 8.0, 8.0, "#ffffff", 1.0, 0.0, 0.0),
            PaintCommand::PopColorMatrix,
        ],
    );
    assert_eq!(requirements(&filtered.plan), vec![Requirement::ColorMatrix]);
    assert!(filtered.plan.passes.iter().any(|pass| matches!(
        pass.kind,
        PassKind::Filter {
            shader: ShaderId::ColorMatrix,
            ..
        }
    )));
}

#[test]
fn large_rounded_clip_has_no_seams_and_rounds_every_corner() {
    let mut backend = RefBackend::new();
    let extent = Extent::new(400, 300);
    let case = build(
        "large_rounded_clip",
        extent,
        Oklch::TRANSPARENT,
        &[
            PaintCommand::PushRoundedClip {
                rect: rect(4.0, 4.0, 380.0, 280.0),
                radius: 12.0,
            },
            quad(0.0, 0.0, 400.0, 300.0, "#3366cc", 1.0, 0.0, 0.0),
            quad(150.0, 0.0, 100.0, 120.0, "#ffffff", 0.5, 0.0, 0.0),
            PaintCommand::PopClip,
        ],
    );
    let pixels = Session::new(&mut backend)
        .render_case(&case)
        .expect("renders");
    let column = |x: usize| (30..270).map(|y| at(&pixels, x, y)).collect::<Vec<_>>();
    (20..370).for_each(|x| {
        [14, 15, 16, 17, 266, 267, 268, 269]
            .into_iter()
            .for_each(|y| assert_eq!(at(&pixels, x, y)[3], 255, "alpha at {x},{y}"));
    });
    (30..270).for_each(|y| {
        [14, 15, 16, 17, 370, 371, 372, 373]
            .into_iter()
            .for_each(|x| assert_eq!(at(&pixels, x, y)[3], 255, "alpha at {x},{y}"));
    });
    assert_eq!(at(&pixels, 190, 20), at(&pixels, 190, 14));
    assert_eq!(at(&pixels, 190, 16), at(&pixels, 190, 15));
    assert_eq!(column(190)[0], at(&pixels, 190, 30));
    [(4, 4), (383, 4), (4, 283), (383, 283)]
        .into_iter()
        .for_each(|(x, y)| assert_eq!(at(&pixels, x, y)[3], 0, "corner {x},{y}"));
    [(7, 7), (380, 7), (7, 280), (380, 280)]
        .into_iter()
        .for_each(|(x, y)| {
            let alpha = at(&pixels, x, y)[3];
            assert!(alpha > 0 && alpha < 255, "edge {x},{y}: {alpha}");
        });
}

#[test]
fn isolated_custom_effects_keep_backdrop_blur_inside_their_own_frame() {
    for (isolate_backdrop, expected) in [(false, [0, 0, 255, 255]), (true, [255, 0, 0, 255])] {
        let mut backend = RefBackend::new();
        let bounds = EXTENT.bounds();
        let pixels = render(
            &mut backend,
            Oklch::TRANSPARENT,
            &[
                quad(0.0, 0.0, 48.0, 32.0, "#0000ff", 1.0, 0.0, 0.0),
                PaintCommand::PushCustomEffect {
                    bounds,
                    spec: CustomPassSpec {
                        isolate_backdrop,
                        reference: None,
                        name: INVERT.into(),
                        hlsl: String::new(),
                        entry: "main".into(),
                        bounds,
                        params: vec![0.0],
                    },
                },
                quad(0.0, 0.0, 48.0, 32.0, "#ff0000", 1.0, 0.0, 0.0),
                backdrop_blur(rect(8.0, 8.0, 32.0, 16.0), 3.0, 4.0),
                PaintCommand::PopCustomEffect,
            ],
        );
        assert_eq!(
            at(&pixels, 24, 16),
            expected,
            "isolated: {isolate_backdrop}"
        );
    }
}
