//! Portable fallback: no native compositor integration.
//!
//! Every material except `None` reports an explicit fallback so callers can
//! keep readable contrast instead of assuming blur.

use crate::material::{MaterialCapability, WindowMaterial};

/// Human-readable backend id for diagnostics.
pub fn backend_name() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "winit-portable-linux (no native material)"
    }
    #[cfg(target_os = "macos")]
    {
        "winit-portable-macos (no native material; NSVisualEffectView future)"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        "winit-portable-fallback (no native material)"
    }
}

/// Always falls back to opaque; never claims OS blur. The `hwnd` argument
/// does not exist here by design — there is no native handle to drive.
pub fn apply_material(requested: WindowMaterial) -> MaterialCapability {
    if requested == WindowMaterial::None {
        return MaterialCapability {
            requested,
            supported: true,
            active: WindowMaterial::None,
            reason: "default opaque frame",
        };
    }
    MaterialCapability::fallback(
        requested,
        "native window materials are Windows-11-only in this milestone; using opaque frame",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_explicit() {
        assert!(!backend_name().is_empty());
        let c = apply_material(WindowMaterial::Acrylic);
        assert!(!c.supported);
        assert_eq!(c.active, WindowMaterial::None);
    }
}
