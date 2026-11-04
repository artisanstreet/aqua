use crate::prefs::{prefers_opaque_frame, SystemPrefs};
use crate::shell::WindowMaterial;

pub const DWM_BACKDROP_MIN_BUILD: u32 = 22621;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum DwmSystemBackdrop {
    Auto = 0,
    None = 1,
    MainWindow = 2,
    TransientWindow = 3,
    TabbedWindow = 4,
}

impl DwmSystemBackdrop {
    pub fn of(material: WindowMaterial) -> Self {
        match material {
            WindowMaterial::Solid => DwmSystemBackdrop::None,
            WindowMaterial::Mica => DwmSystemBackdrop::MainWindow,
            WindowMaterial::Tabbed => DwmSystemBackdrop::TabbedWindow,
            WindowMaterial::Acrylic => DwmSystemBackdrop::TransientWindow,
        }
    }
}

pub fn needs_alpha_surface(material: WindowMaterial) -> bool {
    material != WindowMaterial::Solid
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialDecision {
    Apply(WindowMaterial),
    Fallback(&'static str),
}

pub fn decide(
    requested: WindowMaterial,
    build: Option<u32>,
    prefs: &SystemPrefs,
) -> MaterialDecision {
    match (requested, build, prefers_opaque_frame(prefs)) {
        (WindowMaterial::Solid, _, _) => MaterialDecision::Apply(WindowMaterial::Solid),
        (_, _, true) => MaterialDecision::Fallback("high contrast or transparency effects off"),
        (_, Some(build), _) if build < DWM_BACKDROP_MIN_BUILD => {
            MaterialDecision::Fallback("system backdrop needs Windows 11 build 22621")
        }
        (material, _, _) => MaterialDecision::Apply(material),
    }
}
