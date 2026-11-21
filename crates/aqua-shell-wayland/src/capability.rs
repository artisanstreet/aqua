use aqua_protocol::prefs::SystemPrefs;
use aqua_protocol::{MaterialCapability, WindowMaterial};

pub fn material_capability(requested: WindowMaterial) -> MaterialCapability {
    match requested {
        WindowMaterial::Solid => MaterialCapability::available(requested, "default opaque frame"),
        _ => MaterialCapability::fallback(
            requested,
            "no system backdrop protocol on this compositor",
        ),
    }
}

pub fn query_preferences() -> SystemPrefs {
    aqua_shell::fallback::capability::query_preferences()
}
