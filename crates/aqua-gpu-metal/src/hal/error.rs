use super::selection::AdapterPreference;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no usable Metal device matches {preference:?}; found: {found}")]
    NoAdapter {
        preference: AdapterPreference,
        found: String,
    },
    #[error("{call} returned nothing")]
    Unavailable { call: &'static str },
    #[error("MSL for {entry} is missing: aqua-shaders was built without SPIR-V and SPIRV-Cross")]
    MissingShader { entry: &'static str },
    #[error("compiling the MSL for {entry} failed: {reason}")]
    Library { entry: &'static str, reason: String },
    #[error("the MSL library for {entry} has no function of that name")]
    MissingFunction { entry: &'static str },
    #[error("building the {pipeline} pipeline failed: {reason}")]
    Pipeline { pipeline: String, reason: String },
    #[error("the device was lost during {call}")]
    DeviceLost { call: &'static str },
    #[error("the {label} command buffer failed: {reason}")]
    CommandBuffer { label: &'static str, reason: String },
}

impl Error {
    pub fn is_device_lost(&self) -> bool {
        matches!(self, Error::DeviceLost { .. })
    }
}
