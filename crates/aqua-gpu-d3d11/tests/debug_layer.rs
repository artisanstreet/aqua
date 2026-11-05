#![cfg(windows)]

use std::sync::Mutex;

use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::conformance::{Session, cases};

static SERIAL: Mutex<()> = Mutex::new(());

fn validation_messages(preference: AdapterPreference, format: SurfaceFormat) -> Vec<String> {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("AQUA_D3D11_DEBUG", "1");
    let mut backend = OffscreenBackend::with_format(preference, format)
        .unwrap_or_else(|error| panic!("adapter {preference:?} unavailable: {error}"));
    assert!(
        backend.debug_layer(),
        "the D3D11 SDK layers (d3d11_3SDKLayers.dll) must be installed for this test"
    );
    let mut found = backend.debug_errors();
    let mut total = found.len();
    for case in cases() {
        let mut session = Session::new(&mut backend);
        let first = session.render_case(&case);
        let second = session.render(&case.plan);
        if let Err(error) = first.and(second) {
            found.push(format!("[{}] render failed: {error}", case.name));
        }
        let messages = backend.debug_errors();
        total += messages.len();
        found.extend(
            messages
                .into_iter()
                .map(|message| format!("{preference:?} [{}]: {message}", case.name)),
        );
    }
    eprintln!("{preference:?} {format:?}: {total} debug-layer messages");
    found
}

fn assert_clean(messages: Vec<String>) {
    let mut unique = messages;
    unique.sort();
    unique.dedup();
    assert!(unique.is_empty(), "{}", unique.join("\n"));
}

#[test]
fn warp_scene_is_clean_under_the_debug_layer() {
    assert_clean(validation_messages(
        AdapterPreference::Warp,
        SurfaceFormat::Rgba16Float,
    ));
}

#[test]
fn hardware_scene_is_clean_under_the_debug_layer() {
    assert_clean(validation_messages(
        AdapterPreference::HighPerformance,
        SurfaceFormat::Rgba16Float,
    ));
}

#[test]
fn hardware_unorm_scene_is_clean_under_the_debug_layer() {
    assert_clean(validation_messages(
        AdapterPreference::HighPerformance,
        SurfaceFormat::Bgra8Unorm,
    ));
}
