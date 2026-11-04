use aqua_protocol::{FeatureTier, GpuBackend, GpuReadback};

use super::cases::{Case, cases};
use super::compare::{Comparison, Tolerance, compare};
use super::contract::{ContractReport, run_contract};
use super::fuzz::fuzz_cases;
use super::session::Session;

pub const UNSUPPORTED_MARKER: &str = "unsupported by this backend";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuiteConfig {
    pub tolerance: Tolerance,
    pub fuzz_seed: u64,
    pub fuzz_count: usize,
    pub tolerate_unsupported: bool,
}

impl Default for SuiteConfig {
    fn default() -> Self {
        Self {
            tolerance: Tolerance::default(),
            fuzz_seed: 0xA90A_C0DE,
            fuzz_count: 48,
            tolerate_unsupported: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Compared(Comparison),
    Errored(String),
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaseReport {
    pub name: String,
    pub outcome: Outcome,
}

impl CaseReport {
    pub fn passed(&self) -> bool {
        match &self.outcome {
            Outcome::Compared(comparison) => comparison.passed(),
            Outcome::Errored(_) => false,
            Outcome::Unsupported(_) => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SuiteReport {
    pub backend: String,
    pub adapter: String,
    pub tier: FeatureTier,
    pub cases: Vec<CaseReport>,
    pub contract: Vec<ContractReport>,
}

impl SuiteReport {
    pub fn passed(&self) -> bool {
        self.cases.iter().all(CaseReport::passed)
            && self.contract.iter().all(ContractReport::passed)
    }

    pub fn failures(&self) -> Vec<String> {
        let cases =
            self.cases
                .iter()
                .filter(|case| !case.passed())
                .map(|case| match &case.outcome {
                    Outcome::Errored(message) | Outcome::Unsupported(message) => {
                        format!("{}: {message}", case.name)
                    }
                    Outcome::Compared(comparison) => format!(
                        "{}: {} samples over tolerance (interior max {}, edge max {}), first {:?}",
                        case.name,
                        comparison.failing_samples,
                        comparison.interior_max,
                        comparison.edge_max,
                        comparison.failures.first()
                    ),
                });
        let contract = self.contract.iter().filter_map(|check| {
            check
                .failure
                .as_ref()
                .map(|message| format!("contract {}: {message}", check.name))
        });
        cases.chain(contract).collect()
    }

    pub fn worst_interior(&self) -> u8 {
        self.compared().map(|c| c.interior_max).max().unwrap_or(0)
    }

    pub fn worst_soft(&self) -> u8 {
        self.compared().map(|c| c.soft_max).max().unwrap_or(0)
    }

    pub fn worst_edge(&self) -> u8 {
        self.compared().map(|c| c.edge_max).max().unwrap_or(0)
    }

    fn compared(&self) -> impl Iterator<Item = &Comparison> {
        self.cases.iter().filter_map(|case| match &case.outcome {
            Outcome::Compared(comparison) => Some(comparison),
            Outcome::Errored(_) | Outcome::Unsupported(_) => None,
        })
    }
}

impl std::fmt::Display for SuiteReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "conformance {} / {} ({:?}): {} cases, {} contract checks, worst interior {}, soft {}, edge {}",
            self.backend,
            self.adapter,
            self.tier,
            self.cases.len(),
            self.contract.len(),
            self.worst_interior(),
            self.worst_soft(),
            self.worst_edge()
        )?;
        for case in &self.cases {
            match &case.outcome {
                Outcome::Compared(c) => writeln!(
                    f,
                    "  {} {}: interior {} px max {}, soft {} px max {}, edge {} px max {}",
                    if c.passed() { "ok  " } else { "FAIL" },
                    case.name,
                    c.interior_pixels,
                    c.interior_max,
                    c.soft_pixels,
                    c.soft_max,
                    c.edge_pixels,
                    c.edge_max
                )?,
                Outcome::Errored(message) => writeln!(f, "  ERR  {}: {message}", case.name)?,
                Outcome::Unsupported(message) => writeln!(f, "  SKIP {}: {message}", case.name)?,
            }
        }
        for check in &self.contract {
            match &check.failure {
                None => writeln!(f, "  ok   contract {}", check.name)?,
                Some(message) => writeln!(f, "  FAIL contract {}: {message}", check.name)?,
            }
        }
        Ok(())
    }
}

pub fn run_case<R, B>(
    reference: &mut R,
    backend: &mut B,
    case: &Case,
    tolerance: Tolerance,
) -> Result<Comparison, String>
where
    R: GpuBackend + GpuReadback,
    B: GpuBackend + GpuReadback,
{
    let expected = Session::new(reference)
        .render_case(case)
        .map_err(|e| format!("reference: {e}"))?;
    let actual = Session::new(backend)
        .render_case(case)
        .map_err(|e| format!("backend: {e}"))?;
    compare_pixels(case, &expected, &actual, tolerance)
}

fn compare_pixels(
    case: &Case,
    expected: &aqua_protocol::SurfacePixels,
    actual: &aqua_protocol::SurfacePixels,
    tolerance: Tolerance,
) -> Result<Comparison, String> {
    match (
        expected.extent == actual.extent,
        expected.rgba8.len() == actual.rgba8.len(),
    ) {
        (true, true) => Ok(compare(&case.plan, expected, actual, tolerance)),
        _ => Err("readback size differs between backend and reference".into()),
    }
}

pub fn all_cases(config: &SuiteConfig) -> Vec<Case> {
    cases()
        .into_iter()
        .chain(fuzz_cases(config.fuzz_seed, config.fuzz_count))
        .collect()
}

pub fn run_suite<R, B>(reference: &mut R, backend: &mut B, config: &SuiteConfig) -> SuiteReport
where
    R: GpuBackend + GpuReadback,
    B: GpuBackend + GpuReadback,
{
    let mut reference_session = Session::new(reference);
    let mut backend_session = Session::new(backend);
    let reports = all_cases(config)
        .into_iter()
        .map(|case| {
            let expected = reference_session
                .render_case(&case)
                .map_err(|e| format!("reference: {e}"));
            let actual = backend_session
                .render_case(&case)
                .map_err(|e| format!("backend: {e}"));
            let outcome = expected
                .and_then(|e| actual.map(|a| (e, a)))
                .and_then(|(e, a)| compare_pixels(&case, &e, &a, config.tolerance))
                .map_or_else(
                    |message| match config.tolerate_unsupported
                        && message.contains(UNSUPPORTED_MARKER)
                    {
                        true => Outcome::Unsupported(message),
                        false => Outcome::Errored(message),
                    },
                    Outcome::Compared,
                );
            CaseReport {
                name: case.name,
                outcome,
            }
        })
        .collect();
    let capabilities = backend_session.backend().capabilities().clone();
    let contract = run_contract(backend);
    SuiteReport {
        backend: capabilities.backend,
        adapter: capabilities.adapter,
        tier: capabilities.tier,
        cases: reports,
        contract,
    }
}
