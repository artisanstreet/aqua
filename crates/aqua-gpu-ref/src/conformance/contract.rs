use aqua_graph::{FramePlan, PassKind};
use aqua_protocol::{
    Features, FrameInputs, GpuBackend, GpuReadback, SurfaceId, SurfaceTarget, UploadBatch,
};

use super::cases::{cases, Case, STANDARD};
use super::effects::requirements;
use super::session::Session;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractReport {
    pub name: &'static str,
    pub failure: Option<String>,
}

impl ContractReport {
    pub fn passed(&self) -> bool {
        self.failure.is_none()
    }
}

fn check(name: &'static str, result: Result<(), String>) -> ContractReport {
    ContractReport {
        name,
        failure: result.err(),
    }
}

fn expect(condition: bool, message: impl FnOnce() -> String) -> Result<(), String> {
    match condition {
        true => Ok(()),
        false => Err(message()),
    }
}

fn describe<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
}

fn standard_cases(features: &Features) -> Vec<Case> {
    cases()
        .into_iter()
        .filter(|case| case.plan.extent == STANDARD)
        .filter(|case| {
            requirements(&case.plan)
                .iter()
                .all(|requirement| requirement.offered_by(features))
        })
        .collect()
}

fn poisoned(plan: &FramePlan) -> FramePlan {
    let mut plan = plan.clone();
    for pass in &mut plan.passes {
        if let PassKind::Instanced { instances, .. } = &mut pass.kind {
            if let Some(instance) = instances.first_mut() {
                instance.bounds.x = f32::NAN;
                return plan;
            }
        }
    }
    plan.passes.clear();
    plan
}

fn capabilities_are_reported<B: GpuBackend + GpuReadback>(backend: &mut B) -> Result<(), String> {
    let capabilities = backend.capabilities();
    expect(
        !capabilities.backend.is_empty() && !capabilities.adapter.is_empty(),
        || "backend and adapter names must be non-empty".into(),
    )?;
    expect(capabilities.max_texture_extent >= STANDARD.width, || {
        format!(
            "max_texture_extent {} is below the conformance extent",
            capabilities.max_texture_extent
        )
    })
}

fn unknown_surface_is_rejected<B: GpuBackend + GpuReadback>(backend: &mut B) -> Result<(), String> {
    let plan = &standard_cases(&backend.capabilities().features)[0].plan;
    let result = backend.execute(
        plan,
        &FrameInputs {
            surface: SurfaceId(u32::MAX),
        },
    );
    expect(result.is_err(), || {
        "execute accepted a surface that was never created".into()
    })?;
    expect(backend.read_surface(SurfaceId(u32::MAX)).is_err(), || {
        "read_surface accepted a surface that was never created".into()
    })
}

fn extent_mismatch_is_rejected<B: GpuBackend + GpuReadback>(backend: &mut B) -> Result<(), String> {
    let plan = &standard_cases(&backend.capabilities().features)[0].plan;
    let small = aqua_graph::Extent {
        width: 16,
        height: 16,
    };
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(small))
        .map_err(describe)?;
    expect(
        backend.execute(plan, &FrameInputs { surface }).is_err(),
        || "execute accepted a plan whose extent differs from the surface".into(),
    )
}

fn invalid_plan_is_rejected<B: GpuBackend + GpuReadback>(backend: &mut B) -> Result<(), String> {
    let plan = poisoned(&standard_cases(&backend.capabilities().features)[0].plan);
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(plan.extent))
        .map_err(describe)?;
    expect(
        backend.execute(&plan, &FrameInputs { surface }).is_err(),
        || "execute accepted a plan with a non-finite value".into(),
    )
}

fn empty_upload_is_accepted<B: GpuBackend + GpuReadback>(backend: &mut B) -> Result<(), String> {
    backend.upload(UploadBatch::default()).map_err(describe)
}

fn rendering_is_deterministic<B: GpuBackend + GpuReadback>(backend: &mut B) -> Result<(), String> {
    let mut session = Session::new(backend);
    let features = session.backend().capabilities().features;
    for case in standard_cases(&features) {
        let first = session.render_case(&case).map_err(describe)?;
        let second = session.render_case(&case).map_err(describe)?;
        expect(first == second, || {
            format!("{} differs between two executions", case.name)
        })?;
    }
    Ok(())
}

fn surface_reuse_leaves_no_residue<B: GpuBackend + GpuReadback>(
    backend: &mut B,
) -> Result<(), String> {
    let all = standard_cases(&backend.capabilities().features);
    let mut session = Session::new(backend);
    let baseline: Vec<_> = all
        .iter()
        .map(|case| session.render_case(case).map_err(describe))
        .collect::<Result<_, _>>()?;
    for (index, case) in all.iter().enumerate().rev() {
        let again = session.render_case(case).map_err(describe)?;
        expect(again == baseline[index], || {
            format!(
                "{} changed after other plans ran on the same surface",
                case.name
            )
        })?;
    }
    Ok(())
}

fn recovery_preserves_output<B: GpuBackend + GpuReadback>(backend: &mut B) -> Result<(), String> {
    let all = standard_cases(&backend.capabilities().features);
    let before = {
        let mut session = Session::new(backend);
        all.iter()
            .map(|case| session.render_case(case).map_err(describe))
            .collect::<Result<Vec<_>, _>>()?
    };
    backend.recover().map_err(describe)?;
    let mut session = Session::new(backend);
    for (case, expected) in all.iter().zip(&before) {
        let after = session.render_case(case).map_err(describe)?;
        expect(&after == expected, || {
            format!("{} differs after recover", case.name)
        })?;
    }
    Ok(())
}

pub fn run_contract<B: GpuBackend + GpuReadback>(backend: &mut B) -> Vec<ContractReport> {
    vec![
        check(
            "capabilities_are_reported",
            capabilities_are_reported(backend),
        ),
        check(
            "unknown_surface_is_rejected",
            unknown_surface_is_rejected(backend),
        ),
        check(
            "extent_mismatch_is_rejected",
            extent_mismatch_is_rejected(backend),
        ),
        check(
            "invalid_plan_is_rejected",
            invalid_plan_is_rejected(backend),
        ),
        check(
            "empty_upload_is_accepted",
            empty_upload_is_accepted(backend),
        ),
        check(
            "rendering_is_deterministic",
            rendering_is_deterministic(backend),
        ),
        check(
            "surface_reuse_leaves_no_residue",
            surface_reuse_leaves_no_residue(backend),
        ),
        check(
            "recovery_preserves_output",
            recovery_preserves_output(backend),
        ),
    ]
}
