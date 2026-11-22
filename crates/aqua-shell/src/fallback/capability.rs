use aqua_protocol::prefs::SystemPrefs;
use aqua_protocol::{MaterialCapability, WindowMaterial};

pub fn material_capability(requested: WindowMaterial) -> MaterialCapability {
    match requested {
        WindowMaterial::Solid => MaterialCapability::available(requested, "default opaque frame"),
        _ => MaterialCapability::fallback(requested, "native material path is Windows-only"),
    }
}

pub fn query_preferences() -> SystemPrefs {
    SystemPrefs::fallback().with_overrides(|name| std::env::var(name).ok())
}
