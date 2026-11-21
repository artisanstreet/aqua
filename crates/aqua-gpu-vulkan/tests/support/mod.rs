use aqua_gpu_vulkan::hal::Error as DeviceError;
use aqua_gpu_vulkan::scene::{
    AdapterPreference, Descriptors, DeviceOptions, Error, OffscreenBackend, OffscreenOptions,
    SurfaceFormat, Validation,
};

fn required() -> bool {
    std::env::var_os("AQUA_REQUIRE_GPU").is_some_and(|value| value == "1")
}

#[allow(dead_code)]
pub fn options(preference: AdapterPreference, descriptors: Descriptors) -> DeviceOptions {
    DeviceOptions {
        preference,
        validation: Validation::On,
        descriptors,
    }
}

#[allow(dead_code)]
pub fn open_with(options: OffscreenOptions) -> Option<OffscreenBackend> {
    let preference = options.device.preference.clone();
    match OffscreenBackend::new(options) {
        Ok(backend) => Some(backend),
        Err(Error::Device(
            error @ (DeviceError::NoAdapter { .. }
            | DeviceError::Loader(_)
            | DeviceError::MissingLayer { .. }),
        )) => {
            eprintln!("SKIP-NO-ADAPTER: {preference:?}: {error}");
            assert!(
                preference == AdapterPreference::Hardware || !required(),
                "AQUA_REQUIRE_GPU=1 but {preference:?} is unavailable: {error}"
            );
            None
        }
        Err(error) => panic!("{preference:?} backend failed to start: {error}"),
    }
}

#[allow(dead_code)]
pub fn open(preference: AdapterPreference, format: SurfaceFormat) -> Option<OffscreenBackend> {
    open_with(OffscreenOptions {
        device: options(preference, Descriptors::Automatic),
        format,
        ..OffscreenOptions::default()
    })
}

#[allow(dead_code)]
pub fn assert_clean(backend: &OffscreenBackend) {
    assert!(backend.validating(), "the validation layer is active");
    let errors = backend.validation_errors();
    assert!(
        errors.is_empty(),
        "{} validation errors, first: {}",
        errors.len(),
        errors
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
