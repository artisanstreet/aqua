use aqua_protocol::prefs::SystemPrefs;
use aqua_protocol::{MaterialCapability, WindowMaterial};

pub fn material_capability(requested: WindowMaterial) -> MaterialCapability {
    match (requested, cfg!(windows)) {
        (WindowMaterial::Solid, _) => {
            MaterialCapability::available(requested, "default opaque frame")
        }
        (_, true) => MaterialCapability::available(
            requested,
            "Windows 11 DWM system-backdrop path; runtime build check at apply",
        ),
        (_, false) => {
            MaterialCapability::fallback(requested, "native material path is Windows-only")
        }
    }
}

pub fn query_preferences() -> SystemPrefs {
    #[cfg(windows)]
    {
        crate::prefs::query()
    }
    #[cfg(not(windows))]
    {
        SystemPrefs::fallback().with_overrides(|name| std::env::var(name).ok())
    }
}
