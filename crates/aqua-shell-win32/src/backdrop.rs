use aqua_platform::material::{DWM_BACKDROP_MIN_BUILD, WindowMaterial};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Apply(WindowMaterial),
    Fallback(&'static str),
}

pub fn decide(requested: WindowMaterial, build: Option<u32>, opaque_frame: bool) -> Decision {
    match (requested, build, opaque_frame) {
        (WindowMaterial::None, _, _) => Decision::Apply(WindowMaterial::None),
        (_, _, true) => Decision::Fallback("high contrast or transparency effects off"),
        (_, Some(build), _) if build < DWM_BACKDROP_MIN_BUILD => {
            Decision::Fallback("system backdrop needs Windows 11 build 22621")
        }
        (material, _, _) => Decision::Apply(material),
    }
}
