use aqua_protocol::prefs::SystemPrefs;
use aqua_protocol::{MaterialCapability, WindowMaterial};

pub fn material_capability(requested: WindowMaterial) -> MaterialCapability {
    match requested {
        WindowMaterial::Solid => MaterialCapability::available(requested, "default opaque window"),
        _ => MaterialCapability::fallback(requested, "NSVisualEffectView is not wired"),
    }
}

pub fn query_preferences() -> SystemPrefs {
    aqua_shell::fallback::capability::query_preferences()
}
