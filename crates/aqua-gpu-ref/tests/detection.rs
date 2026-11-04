use aqua_gpu_ref::RefBackend;
use aqua_gpu_ref::conformance::{Outcome, SuiteConfig, Tolerance, run_suite};
use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    FrameInputs, FrameReport, GpuBackend, GpuCapabilities, GpuError, GpuReadback, PresentHint,
    PresentTimes, RecoveryReport, SurfaceId, SurfacePixels, SurfaceTarget, UploadBatch,
};

struct Perturbed<F: Fn(&mut SurfacePixels)> {
    inner: RefBackend,
    perturb: F,
}

impl<F: Fn(&mut SurfacePixels)> GpuBackend for Perturbed<F> {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        self.inner.capabilities()
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        self.inner.create_surface(target)
    }

    fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
        self.inner.upload(batch)
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        self.inner.execute(plan, inputs)
    }

    fn present(&mut self, surface: SurfaceId, hint: PresentHint) -> Result<PresentTimes, GpuError> {
        self.inner.present(surface, hint)
    }

    fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
        self.inner.recover()
    }
}

impl<F: Fn(&mut SurfacePixels)> GpuReadback for Perturbed<F> {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        let mut pixels = self.inner.read_surface(surface)?;
        (self.perturb)(&mut pixels);
        Ok(pixels)
    }
}

fn config() -> SuiteConfig {
    SuiteConfig {
        fuzz_count: 0,
        ..SuiteConfig::default()
    }
}

fn run(perturb: impl Fn(&mut SurfacePixels)) -> aqua_gpu_ref::conformance::SuiteReport {
    let mut reference = RefBackend::new();
    let mut backend = Perturbed {
        inner: RefBackend::new(),
        perturb,
    };
    run_suite(&mut reference, &mut backend, &config())
}

fn nudge(pixels: &mut SurfacePixels, x: usize, y: usize, channel: usize, delta: i16) {
    if pixels.extent
        != (Extent {
            width: 96,
            height: 72,
        })
    {
        return;
    }
    let index = (y * pixels.extent.width as usize + x) * 4 + channel;
    pixels.rgba8[index] = (pixels.rgba8[index] as i16 + delta).clamp(0, 255) as u8;
}

fn solid_case_is_named(report: &aqua_gpu_ref::conformance::SuiteReport, name: &str) -> bool {
    report
        .cases
        .iter()
        .any(|case| case.name == name && !case.passed())
}

#[test]
fn an_honest_backend_passes_with_the_default_tolerance() {
    let report = run(|_| {});
    assert!(report.passed(), "{}", report.failures().join("\n"));
}

#[test]
fn small_interior_noise_is_tolerated_and_large_noise_is_caught() {
    let quiet = run(|pixels| nudge(pixels, 47, 36, 0, -3));
    assert!(
        quiet.cases.iter().all(|case| case.passed()),
        "{}",
        quiet.failures().join("\n")
    );
    let loud = run(|pixels| nudge(pixels, 47, 36, 0, -40));
    assert!(solid_case_is_named(&loud, "solid_rect"));
    assert!(!loud.passed());
}

#[test]
fn edge_pixels_get_the_wider_tolerance_but_not_an_unbounded_one() {
    let shifted = |delta: i16| run(move |pixels| nudge(pixels, 8, 30, 0, -delta));
    let within = shifted(20);
    assert!(
        within
            .cases
            .iter()
            .filter(|c| c.name == "solid_rect")
            .all(|c| c.passed())
    );
    let beyond = shifted(60);
    assert!(solid_case_is_named(&beyond, "solid_rect"));
}

#[test]
fn a_backend_that_ignores_group_opacity_fails_the_isolation_cases() {
    let report = run(|pixels| {
        let pixel = (36 * 96 + 40) * 4;
        if pixels.extent
            == (Extent {
                width: 96,
                height: 72,
            })
        {
            pixels.rgba8[pixel] = 255;
        }
    });
    assert!(!report.passed());
}

#[test]
fn exact_tolerance_flags_a_single_level_error() {
    let mut reference = RefBackend::new();
    let mut backend = Perturbed {
        inner: RefBackend::new(),
        perturb: |pixels: &mut SurfacePixels| nudge(pixels, 47, 36, 0, -1),
    };
    let report = run_suite(
        &mut reference,
        &mut backend,
        &SuiteConfig {
            tolerance: Tolerance::EXACT,
            ..config()
        },
    );
    assert!(solid_case_is_named(&report, "solid_rect"));
}

#[test]
fn unsupported_operations_fail_unless_explicitly_tolerated() {
    struct NoUploads(RefBackend);
    impl GpuBackend for NoUploads {
        type Error = GpuError;
        fn capabilities(&self) -> &GpuCapabilities {
            self.0.capabilities()
        }
        fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
            self.0.create_surface(target)
        }
        fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
            match batch.textures.is_empty() {
                true => Ok(()),
                false => Err(GpuError::Unsupported("texture uploads")),
            }
        }
        fn execute(
            &mut self,
            plan: &FramePlan,
            inputs: &FrameInputs,
        ) -> Result<FrameReport, GpuError> {
            self.0.execute(plan, inputs)
        }
        fn present(
            &mut self,
            surface: SurfaceId,
            hint: PresentHint,
        ) -> Result<PresentTimes, GpuError> {
            self.0.present(surface, hint)
        }
        fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
            self.0.recover()
        }
    }
    impl GpuReadback for NoUploads {
        fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
            self.0.read_surface(surface)
        }
    }
    let strict = run_suite(
        &mut RefBackend::new(),
        &mut NoUploads(RefBackend::new()),
        &config(),
    );
    assert!(!strict.passed());
    let lenient = run_suite(
        &mut RefBackend::new(),
        &mut NoUploads(RefBackend::new()),
        &SuiteConfig {
            tolerate_unsupported: true,
            ..config()
        },
    );
    assert!(
        lenient
            .cases
            .iter()
            .any(|c| matches!(c.outcome, Outcome::Unsupported(_)))
    );
    assert!(lenient.cases.iter().all(|c| c.passed()));
}
