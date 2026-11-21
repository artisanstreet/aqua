use aqua_gpu_metal::hal::Error as DeviceError;
use aqua_gpu_metal::scene::{Error, OffscreenBackend, OffscreenOptions, SurfaceFormat};

fn required() -> bool {
    std::env::var_os("AQUA_REQUIRE_GPU").is_some_and(|value| value == "1")
}

#[allow(dead_code)]
pub fn open_with(options: OffscreenOptions) -> Option<OffscreenBackend> {
    let preference = options.device.preference.clone();
    match OffscreenBackend::new(options) {
        Ok(backend) => Some(backend),
        Err(Error::Device(error @ DeviceError::NoAdapter { .. })) => {
            eprintln!("SKIP-NO-ADAPTER: {preference:?}: {error}");
            assert!(
                !required(),
                "AQUA_REQUIRE_GPU=1 but no Metal device is usable: {error}"
            );
            None
        }
        Err(error) => panic!("the Metal backend failed to start: {error}"),
    }
}

#[allow(dead_code)]
pub fn open(format: SurfaceFormat) -> Option<OffscreenBackend> {
    open_with(OffscreenOptions {
        format,
        ..OffscreenOptions::default()
    })
}

#[allow(dead_code)]
pub fn assert_clean(backend: &OffscreenBackend) {
    match backend.validating() {
        true => eprintln!("metal validation on: {}", backend.metal_object_classes()),
        false => {
            eprintln!(
                "SKIP-NO-METAL-VALIDATION: run with MTL_DEBUG_LAYER=1 MTL_SHADER_VALIDATION=1 ({})",
                backend.metal_object_classes()
            );
            assert!(
                !required(),
                "AQUA_REQUIRE_GPU=1 but Metal API validation is off ({}): set MTL_DEBUG_LAYER=1",
                backend.metal_object_classes()
            );
        }
    }
    let errors = backend.validation_errors();
    assert!(
        errors.is_empty(),
        "{} Metal errors, first: {}",
        errors.len(),
        errors
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
