#![cfg(windows)]

use std::sync::Mutex;

use aqua_gpu_d3d12::hal::tier::Tier;
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat, TierLimit};
use aqua_gpu_ref::conformance::{cases, Session};

static SERIAL: Mutex<()> = Mutex::new(());

const COMBINATIONS: [(&str, TierLimit); 4] = [
    ("baseline", TierLimit::BASELINE),
    (
        "bindless",
        TierLimit {
            binding: Tier::Bindless,
            enhanced_barriers: false,
        },
    ),
    (
        "enhanced-barriers",
        TierLimit {
            binding: Tier::Baseline,
            enhanced_barriers: true,
        },
    ),
    ("bindless+enhanced-barriers", TierLimit::MAX),
];

fn validation_errors(preference: AdapterPreference, format: SurfaceFormat) -> Vec<String> {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("AQUA_D3D12_DEBUG", "1");
    std::env::set_var("AQUA_D3D12_GBV", "1");
    let mut found = Vec::new();
    for (name, limit) in COMBINATIONS {
        let Ok(mut backend) = OffscreenBackend::with_tiers(preference, format, limit) else {
            assert_ne!(
                preference,
                AdapterPreference::Warp,
                "WARP must be available"
            );
            continue;
        };
        let mut total = 0;
        for case in cases() {
            let mut session = Session::new(&mut backend);
            let _ = session.render_case(&case);
            let _ = session.render(&case.plan);
            let errors = backend.debug_errors();
            total += errors.len();
            found.extend(
                errors
                    .into_iter()
                    .map(|error| format!("{preference:?} {name} [{}]: {error}", case.name)),
            );
        }
        eprintln!("{preference:?} {format:?} {name}: {total} validation errors");
    }
    found
}

fn normalized(message: &str) -> String {
    let mut out = String::new();
    let mut rest = message;
    while let Some(at) = rest.find("0x") {
        out.push_str(&rest[..at]);
        out.push_str("0x*");
        let tail = &rest[at + 2..];
        let end = tail
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    let mut squeezed = String::new();
    let mut digits = false;
    for c in out.chars() {
        match (c.is_ascii_digit(), digits) {
            (true, true) => {}
            (true, false) => {
                squeezed.push('#');
                digits = true;
            }
            (false, _) => {
                squeezed.push(c);
                digits = false;
            }
        }
    }
    squeezed.chars().take(1100).collect()
}

fn assert_clean(errors: Vec<String>) {
    let mut unique: Vec<String> = errors.iter().map(|error| normalized(error)).collect();
    unique.sort();
    unique.dedup();
    assert!(unique.is_empty(), "{}", unique.join("\n"));
}

#[test]
fn warp_scene_passes_validation_on_every_tier() {
    assert_clean(validation_errors(
        AdapterPreference::Warp,
        SurfaceFormat::Rgba16Float,
    ));
}

#[test]
fn hardware_scene_passes_validation_on_every_tier() {
    assert_clean(validation_errors(
        AdapterPreference::HighPerformance,
        SurfaceFormat::Rgba16Float,
    ));
}

#[test]
fn hardware_unorm_scene_passes_validation_on_every_tier() {
    assert_clean(validation_errors(
        AdapterPreference::HighPerformance,
        SurfaceFormat::Bgra8Unorm,
    ));
}
