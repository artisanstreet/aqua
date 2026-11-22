use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use ash::vk;

use super::error::{Checked, Error};

pub(crate) type Sink = Mutex<Vec<String>>;

pub(crate) struct Messenger {
    loader: ash::ext::debug_utils::Instance,
    handle: vk::DebugUtilsMessengerEXT,
    sink: Arc<Sink>,
}

unsafe extern "system" fn collect(
    _severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    sink: *mut c_void,
) -> vk::Bool32 {
    let sink = unsafe { &*(sink as *const Sink) };
    let message = unsafe { data.as_ref() }
        .and_then(|data| unsafe { data.message_as_c_str() })
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_else(|| "validation message without text".to_owned());
    sink.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(message);
    vk::FALSE
}

pub(crate) fn create_info(sink: &Arc<Sink>) -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR)
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
        )
        .pfn_user_callback(Some(collect))
        .user_data(Arc::as_ptr(sink) as *mut c_void)
}

impl Messenger {
    pub fn new(
        entry: &ash::Entry,
        instance: &ash::Instance,
        sink: Arc<Sink>,
    ) -> Result<Self, Error> {
        let loader = ash::ext::debug_utils::Instance::new(entry, instance);
        let handle = unsafe { loader.create_debug_utils_messenger(&create_info(&sink), None) }
            .checked("vkCreateDebugUtilsMessengerEXT")?;
        Ok(Self {
            loader,
            handle,
            sink,
        })
    }

    pub fn drain(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .sink
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    pub fn destroy(&self) {
        unsafe { self.loader.destroy_debug_utils_messenger(self.handle, None) };
    }
}
