use crate::prefs;
use aqua_protocol::material::{decide, DwmSystemBackdrop, MaterialDecision};
use aqua_protocol::WindowMaterial;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::UI::Controls::MARGINS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialOutcome {
    pub requested: WindowMaterial,
    pub active: WindowMaterial,
    pub reason: &'static str,
}

fn set_attribute(
    hwnd: HWND,
    attribute: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE,
    value: i32,
) -> bool {
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            (&value as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        )
    }
    .is_ok()
}

pub(crate) fn extend_frame(hwnd: HWND, full: bool, custom_chrome: bool) {
    let margins = match (full, custom_chrome) {
        (true, _) => MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        },
        (false, true) => MARGINS {
            cxLeftWidth: 0,
            cxRightWidth: 0,
            cyTopHeight: 1,
            cyBottomHeight: 0,
        },
        (false, false) => MARGINS::default(),
    };
    let _ = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
}

pub(crate) fn apply(hwnd: HWND, requested: WindowMaterial, custom_chrome: bool) -> MaterialOutcome {
    let prefs = prefs::query();
    match decide(requested, prefs::build_number(), &prefs) {
        MaterialDecision::Fallback(reason) => {
            set_attribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                DwmSystemBackdrop::None as i32,
            );
            extend_frame(hwnd, false, custom_chrome);
            MaterialOutcome {
                requested,
                active: WindowMaterial::Solid,
                reason,
            }
        }
        MaterialDecision::Apply(material) => {
            let ok = set_attribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                DwmSystemBackdrop::of(material) as i32,
            );
            extend_frame(hwnd, material != WindowMaterial::Solid, custom_chrome);
            match ok {
                true => MaterialOutcome {
                    requested,
                    active: material,
                    reason: "applied",
                },
                false => MaterialOutcome {
                    requested,
                    active: WindowMaterial::Solid,
                    reason: "DwmSetWindowAttribute failed",
                },
            }
        }
    }
}

pub(crate) fn set_dark(hwnd: HWND, dark: bool) -> bool {
    set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, i32::from(dark))
}

pub(crate) fn round_corners(hwnd: HWND) -> bool {
    set_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND.0)
}

pub(crate) fn backdrop_attribute(hwnd: HWND) -> Option<i32> {
    use windows::Win32::Graphics::Dwm::DwmGetWindowAttribute;
    let mut value = 0i32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&mut value as *mut i32).cast(),
            std::mem::size_of::<i32>() as u32,
        )
    }
    .ok()
    .map(|_| value)
}
