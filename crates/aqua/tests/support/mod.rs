#![allow(dead_code)]

use aqua::render::lower::{LowerReport, Lowering};
use aqua_core::Size;
use aqua_gpu_ref::conformance::render as render_plan;
use aqua_gpu_ref::RefBackend;
use aqua_graph::{FramePlan, PlanLimits};
use aqua_protocol::{GpuBackend, GpuError, GpuReadback, Instant, UploadBatch};
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};

pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba8: Vec<u8>,
}

impl Frame {
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let at = (y * self.width + x) * 4;
        [
            self.rgba8[at],
            self.rgba8[at + 1],
            self.rgba8[at + 2],
            self.rgba8[at + 3],
        ]
    }

    pub fn pixels(&self) -> impl Iterator<Item = [u8; 4]> + '_ {
        self.rgba8.as_chunks::<4>().0.iter().copied()
    }
}

trait Device {
    fn limits(&self) -> PlanLimits;
    fn upload(&mut self, batch: UploadBatch);
    fn render(&mut self, plan: &FramePlan) -> Frame;
}

impl<B: GpuBackend<Error = GpuError> + GpuReadback> Device for B {
    fn limits(&self) -> PlanLimits {
        self.capabilities().plan_limits()
    }

    fn upload(&mut self, batch: UploadBatch) {
        GpuBackend::upload(self, batch).unwrap();
    }

    fn render(&mut self, plan: &FramePlan) -> Frame {
        let pixels = render_plan(self, plan).unwrap();
        Frame {
            width: pixels.extent.width as usize,
            height: pixels.extent.height as usize,
            rgba8: pixels.rgba8,
        }
    }
}

pub struct Painter {
    pub label: String,
    device: Box<dyn Device>,
    lowering: Lowering,
    pub report: LowerReport,
}

impl Painter {
    fn new(label: String, device: Box<dyn Device>, fonts: SharedFonts, runs: SharedRuns) -> Self {
        Self {
            label,
            device,
            lowering: Lowering::new(fonts, runs),
            report: LowerReport::default(),
        }
    }

    pub fn register_image(&mut self, layer: u64, width: u32, height: u32, rgba: &[u8]) {
        self.lowering.register_image(layer, width, height, rgba);
    }

    pub fn release_image(&mut self, layer: u64) {
        self.lowering.release_image(layer);
    }

    pub fn paint(&mut self, scene: &Scene, size: Size, scale: f32) -> Frame {
        self.paint_at(scene, size, scale, Instant::ZERO)
    }

    pub fn paint_at(&mut self, scene: &Scene, size: Size, scale: f32, now: Instant) -> Frame {
        self.lowering.at(now);
        let lowered = self
            .lowering
            .lower(scene, size, scale, self.device.limits())
            .unwrap();
        self.report = lowered.report;
        self.device.upload(lowered.uploads);
        self.device.render(&lowered.plan)
    }
}

pub fn reference(fonts: SharedFonts, runs: SharedRuns) -> Painter {
    Painter::new("reference".into(), Box::new(RefBackend::new()), fonts, runs)
}

#[cfg(windows)]
pub fn hardware(fonts: SharedFonts, runs: SharedRuns) -> Vec<Painter> {
    use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend};
    [AdapterPreference::HighPerformance, AdapterPreference::Warp]
        .into_iter()
        .filter_map(|preference| {
            let backend = OffscreenBackend::new(preference).ok()?;
            let label = format!("d3d12 {preference:?} software={}", backend.is_software());
            Some(Painter::new(
                label,
                Box::new(backend),
                fonts.clone(),
                runs.clone(),
            ))
        })
        .collect()
}

#[cfg(not(windows))]
pub fn hardware(_fonts: SharedFonts, _runs: SharedRuns) -> Vec<Painter> {
    Vec::new()
}

pub fn painters(fonts: &SharedFonts, runs: &SharedRuns) -> Vec<Painter> {
    let mut all = vec![reference(fonts.clone(), runs.clone())];
    all.extend(hardware(fonts.clone(), runs.clone()));
    all
}

pub fn frame(ui: &mut aqua::UiApp, now: Instant) {
    ui.advance_clock(now.as_secs_f64());
    ui.build_frame().unwrap();
}
