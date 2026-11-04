use aqua_gpu_ref::conformance::assets::standard_uploads;
use aqua_gpu_ref::conformance::fuzz::{fuzz_scenes, Scene};
use aqua_gpu_ref::conformance::Case;
use aqua_graph::{plan_paint_with, PlanError, PlanLimits};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanVariant {
    Baseline,
    Bindless,
    FineHeapAlignment,
    SmallTextureLimit,
}

pub const VARIANTS: [PlanVariant; 4] = [
    PlanVariant::Baseline,
    PlanVariant::Bindless,
    PlanVariant::FineHeapAlignment,
    PlanVariant::SmallTextureLimit,
];

impl PlanVariant {
    pub fn limits(self) -> PlanLimits {
        let base = PlanLimits::default();
        match self {
            PlanVariant::Baseline => base,
            PlanVariant::Bindless => PlanLimits {
                bindless: true,
                ..base
            },
            PlanVariant::FineHeapAlignment => PlanLimits {
                heap_alignment: 4096,
                ..base
            },
            PlanVariant::SmallTextureLimit => PlanLimits {
                max_texture_extent: 256,
                ..base
            },
        }
    }
}

pub fn variant_case(scene: &Scene, variant: PlanVariant) -> Result<Case, PlanError> {
    plan_paint_with(scene.extent, scene.clear, &scene.commands, variant.limits()).map(|plan| Case {
        name: format!("{}/{variant:?}", scene.name),
        plan,
        uploads: standard_uploads(),
    })
}

pub fn scenes(seed: u64, count: usize) -> Vec<Scene> {
    fuzz_scenes(seed, count)
}
