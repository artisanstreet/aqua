use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_foundation::{NSError, NSString};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandBufferDescriptor, MTLCommandBufferError,
    MTLCommandBufferErrorOption, MTLCommandBufferStatus, MTLCommandQueue, MTLCompileOptions,
    MTLCopyAllDevices, MTLCreateSystemDefaultDevice, MTLDevice, MTLFunction, MTLGPUFamily,
    MTLLanguageVersion, MTLLibrary,
};

use super::budget::MemoryBudget;
use super::error::Error;
use super::selection::{choose, AdapterPreference, Candidate, Fitness};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

const APPLE_FAMILIES: [(u8, MTLGPUFamily); 9] = [
    (9, MTLGPUFamily::Apple9),
    (8, MTLGPUFamily::Apple8),
    (7, MTLGPUFamily::Apple7),
    (6, MTLGPUFamily::Apple6),
    (5, MTLGPUFamily::Apple5),
    (4, MTLGPUFamily::Apple4),
    (3, MTLGPUFamily::Apple3),
    (2, MTLGPUFamily::Apple2),
    (1, MTLGPUFamily::Apple1),
];

const COMMON_FAMILIES: [(u8, MTLGPUFamily); 3] = [
    (3, MTLGPUFamily::Common3),
    (2, MTLGPUFamily::Common2),
    (1, MTLGPUFamily::Common1),
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceOptions {
    pub preference: AdapterPreference,
}

impl DeviceOptions {
    pub fn from_env() -> Self {
        Self {
            preference: AdapterPreference::from_env(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Families {
    pub apple: Option<u8>,
    pub common: Option<u8>,
    pub mac2: bool,
    pub metal3: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub registry_id: u64,
    pub families: Families,
    pub unified_memory: bool,
    pub max_texture_extent: u32,
    pub max_buffer_length: u64,
    pub recommended_working_set: u64,
}

pub(crate) struct InFlight(pub Retained<ProtocolObject<dyn MTLCommandBuffer>>);

unsafe impl Send for InFlight {}

pub struct Device {
    pub(crate) raw: Retained<ProtocolObject<dyn MTLDevice>>,
    pub(crate) queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    info: DeviceInfo,
    validating: bool,
    lost: AtomicBool,
    errors: Mutex<Vec<String>>,
    functions: Mutex<HashMap<&'static str, Retained<ProtocolObject<dyn MTLFunction>>>>,
}

fn families(device: &ProtocolObject<dyn MTLDevice>) -> Families {
    let highest = |table: &[(u8, MTLGPUFamily)]| {
        table
            .iter()
            .find(|(_, family)| device.supportsFamily(*family))
            .map(|(level, _)| *level)
    };
    Families {
        apple: highest(&APPLE_FAMILIES),
        common: highest(&COMMON_FAMILIES),
        mac2: device.supportsFamily(MTLGPUFamily::Mac2),
        metal3: device.supportsFamily(MTLGPUFamily::Metal3),
    }
}

fn fitness(families: Families) -> Fitness {
    match families.common {
        Some(3) => Fitness::Usable,
        _ => Fitness::Lacks("the Common3 GPU family"),
    }
}

fn info(device: &ProtocolObject<dyn MTLDevice>) -> DeviceInfo {
    let families = families(device);
    DeviceInfo {
        name: device.name().to_string(),
        registry_id: device.registryID(),
        families,
        unified_memory: device.hasUnifiedMemory(),
        max_texture_extent: match families.mac2 || families.apple.is_some_and(|level| level >= 3) {
            true => 16_384,
            false => 8_192,
        },
        max_buffer_length: device.maxBufferLength() as u64,
        recommended_working_set: device.recommendedMaxWorkingSetSize(),
    }
}

fn class_name(object: &AnyObject) -> String {
    object.class().name().to_string_lossy().into_owned()
}

pub(crate) fn describe(error: &NSError) -> String {
    format!(
        "{} (domain {}, code {})",
        error.localizedDescription(),
        error.domain(),
        error.code()
    )
}

fn select(
    preference: &AdapterPreference,
) -> Result<Retained<ProtocolObject<dyn MTLDevice>>, Error> {
    let default = MTLCreateSystemDefaultDevice().map(|device| device.registryID());
    let devices = MTLCopyAllDevices();
    let devices: Vec<Retained<ProtocolObject<dyn MTLDevice>>> = devices.iter().collect();
    let candidates: Vec<Candidate> = devices
        .iter()
        .map(|device| Candidate {
            name: device.name().to_string(),
            system_default: default == Some(device.registryID()),
            fitness: fitness(families(device)),
        })
        .collect();
    match choose(preference, &candidates) {
        Some(index) => Ok(devices[index].clone()),
        None => Err(Error::NoAdapter {
            preference: preference.clone(),
            found: match candidates.is_empty() {
                true => "no Metal devices".to_owned(),
                false => candidates
                    .iter()
                    .map(|candidate| match &candidate.fitness {
                        Fitness::Usable => candidate.name.clone(),
                        Fitness::Lacks(what) => format!("{} (lacks {what})", candidate.name),
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            },
        }),
    }
}

impl Device {
    pub fn create(options: &DeviceOptions) -> Result<Arc<Self>, Error> {
        let raw = select(&options.preference)?;
        let queue = raw.newCommandQueue().ok_or(Error::Unavailable {
            call: "newCommandQueue",
        })?;
        let validating = [class_name(raw.as_ref()), class_name(queue.as_ref())]
            .iter()
            .any(|name| name.contains("Debug"));
        Ok(Arc::new(Self {
            info: info(&raw),
            raw,
            queue,
            validating,
            lost: AtomicBool::new(false),
            errors: Mutex::new(Vec::new()),
            functions: Mutex::new(HashMap::new()),
        }))
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn validating(&self) -> bool {
        self.validating
    }

    pub fn object_classes(&self) -> String {
        format!(
            "{} / {}",
            class_name(self.raw.as_ref()),
            class_name(self.queue.as_ref())
        )
    }

    pub fn validation_errors(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.errors))
    }

    pub fn inject_loss(&self) {
        self.lost.store(true, Ordering::SeqCst);
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    pub(crate) fn check(&self, call: &'static str) -> Result<(), Error> {
        match self.is_lost() {
            true => Err(Error::DeviceLost { call }),
            false => Ok(()),
        }
    }

    pub fn memory_budget(&self) -> MemoryBudget {
        MemoryBudget {
            budget: self.info.recommended_working_set,
            usage: self.raw.currentAllocatedSize() as u64,
        }
    }

    pub(crate) fn command_buffer(
        &self,
        label: &'static str,
    ) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>, Error> {
        self.check(label)?;
        let descriptor = MTLCommandBufferDescriptor::new();
        descriptor.setErrorOptions(MTLCommandBufferErrorOption::EncoderExecutionStatus);
        let command =
            self.queue
                .commandBufferWithDescriptor(&descriptor)
                .ok_or(Error::Unavailable {
                    call: "commandBufferWithDescriptor",
                })?;
        command.setLabel(Some(&NSString::from_str(label)));
        Ok(command)
    }

    pub(crate) fn settle(
        &self,
        label: &'static str,
        command: &ProtocolObject<dyn MTLCommandBuffer>,
    ) -> Result<Option<Duration>, Error> {
        command.waitUntilCompleted();
        match command.status() {
            MTLCommandBufferStatus::Completed => {
                let seconds = command.GPUEndTime() - command.GPUStartTime();
                Ok((seconds > 0.0).then(|| Duration::from_secs_f64(seconds)))
            }
            MTLCommandBufferStatus::Error => {
                let error = command.error();
                let reason = error
                    .as_deref()
                    .map_or_else(|| "no error object".to_owned(), describe);
                lock(&self.errors).push(format!("{label}: {reason}"));
                let removed = error.is_some_and(|error| {
                    error.code() == MTLCommandBufferError::DeviceRemoved.0 as isize
                });
                match removed {
                    true => {
                        self.lost.store(true, Ordering::SeqCst);
                        Err(Error::DeviceLost { call: label })
                    }
                    false => Err(Error::CommandBuffer { label, reason }),
                }
            }
            status => unreachable!(
                "invariant: waitUntilCompleted returned with the {label} command buffer in status {}",
                status.0
            ),
        }
    }

    pub(crate) fn function(
        &self,
        entry: &'static str,
    ) -> Result<Retained<ProtocolObject<dyn MTLFunction>>, Error> {
        if let Some(function) = lock(&self.functions).get(entry) {
            return Ok(function.clone());
        }
        let blob = aqua_shaders::find_msl(entry).ok_or(Error::MissingShader { entry })?;
        let source = std::str::from_utf8(blob.bytes)
            .unwrap_or_else(|_| panic!("invariant: aqua-shaders writes {entry} as UTF-8 MSL"));
        let options = MTLCompileOptions::new();
        options.setLanguageVersion(MTLLanguageVersion::Version2_4);
        #[allow(deprecated)]
        options.setFastMathEnabled(false);
        let library: Retained<ProtocolObject<dyn MTLLibrary>> = self
            .raw
            .newLibraryWithSource_options_error(&NSString::from_str(source), Some(&options))
            .map_err(|error| Error::Library {
                entry,
                reason: describe(&error),
            })?;
        let function = library
            .newFunctionWithName(&NSString::from_str(entry))
            .ok_or(Error::MissingFunction { entry })?;
        lock(&self.functions).insert(entry, function.clone());
        Ok(function)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
