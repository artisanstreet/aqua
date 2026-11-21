use std::ffi::CStr;
use std::sync::Arc;

use ash::vk;

use super::debug::{create_info, Messenger, Sink};
use super::error::{Checked, Error};

pub const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";
const VALIDATION_VARIABLE: &str = "AQUA_VULKAN_VALIDATION";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Validation {
    #[default]
    Off,
    On,
}

impl Validation {
    pub fn from_env() -> Self {
        match std::env::var(VALIDATION_VARIABLE).as_deref() {
            Ok("1") => Validation::On,
            _ => Validation::Off,
        }
    }
}

pub(crate) struct Instance {
    _loader: ash::Entry,
    pub raw: ash::Instance,
    messenger: Option<Messenger>,
}

fn layer_available(entry: &ash::Entry, layer: &CStr) -> Result<bool, Error> {
    let layers = unsafe { entry.enumerate_instance_layer_properties() }
        .checked("vkEnumerateInstanceLayerProperties")?;
    Ok(layers
        .iter()
        .any(|properties| properties.layer_name_as_c_str() == Ok(layer)))
}

impl Instance {
    pub fn new(validation: Validation) -> Result<Self, Error> {
        let entry = unsafe { ash::Entry::load() }?;
        if validation == Validation::On && !layer_available(&entry, VALIDATION_LAYER)? {
            return Err(Error::MissingLayer {
                layer: "VK_LAYER_KHRONOS_validation",
            });
        }
        let application = vk::ApplicationInfo::default()
            .application_name(c"aqua")
            .engine_name(c"aqua")
            .api_version(vk::API_VERSION_1_3);
        let sink: Arc<Sink> = Arc::default();
        let (layers, extensions): (Vec<*const i8>, Vec<*const i8>) = match validation {
            Validation::On => (
                vec![VALIDATION_LAYER.as_ptr()],
                vec![
                    ash::ext::debug_utils::NAME.as_ptr(),
                    ash::ext::validation_features::NAME.as_ptr(),
                ],
            ),
            Validation::Off => (Vec::new(), Vec::new()),
        };
        let mut messages = create_info(&sink);
        let enabled = [vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION];
        let mut features =
            vk::ValidationFeaturesEXT::default().enabled_validation_features(&enabled);
        let mut info = vk::InstanceCreateInfo::default()
            .application_info(&application)
            .enabled_layer_names(&layers)
            .enabled_extension_names(&extensions);
        if validation == Validation::On {
            info = info.push_next(&mut messages).push_next(&mut features);
        }
        let raw = unsafe { entry.create_instance(&info, None) }.checked("vkCreateInstance")?;
        let messenger = match validation {
            Validation::On => match Messenger::new(&entry, &raw, sink) {
                Ok(messenger) => Some(messenger),
                Err(error) => {
                    unsafe { raw.destroy_instance(None) };
                    return Err(error);
                }
            },
            Validation::Off => None,
        };
        Ok(Self {
            _loader: entry,
            raw,
            messenger,
        })
    }

    pub fn validating(&self) -> bool {
        self.messenger.is_some()
    }

    pub fn validation_errors(&self) -> Vec<String> {
        self.messenger
            .as_ref()
            .map(Messenger::drain)
            .unwrap_or_default()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if let Some(messenger) = &self.messenger {
            messenger.destroy();
        }
        unsafe { self.raw.destroy_instance(None) };
    }
}
