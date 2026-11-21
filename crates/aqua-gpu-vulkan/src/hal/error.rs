use ash::prelude::VkResult;
use ash::vk;

use super::selection::AdapterPreference;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the Vulkan loader could not be opened: {0}")]
    Loader(#[from] ash::LoadingError),
    #[error("{call} failed: {result}")]
    Vulkan {
        call: &'static str,
        result: vk::Result,
    },
    #[error("the device was lost during {call}")]
    DeviceLost { call: &'static str },
    #[error("no Vulkan 1.3 device matches {preference:?}; found: {found}")]
    NoAdapter {
        preference: AdapterPreference,
        found: String,
    },
    #[error("validation was requested but {layer} is not installed")]
    MissingLayer { layer: &'static str },
    #[error("no memory type with {wanted:?} can hold the {purpose}")]
    NoMemoryType {
        purpose: &'static str,
        wanted: vk::MemoryPropertyFlags,
    },
}

impl Error {
    pub fn is_device_lost(&self) -> bool {
        matches!(self, Error::DeviceLost { .. })
    }
}

pub(crate) trait Checked<T> {
    fn checked(self, call: &'static str) -> Result<T, Error>;
}

impl<T> Checked<T> for VkResult<T> {
    fn checked(self, call: &'static str) -> Result<T, Error> {
        self.map_err(|result| match result {
            vk::Result::ERROR_DEVICE_LOST => Error::DeviceLost { call },
            result => Error::Vulkan { call, result },
        })
    }
}
