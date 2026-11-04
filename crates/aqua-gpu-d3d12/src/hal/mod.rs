pub mod budget;
pub mod recovery;
pub mod ring;
pub mod selection;
pub mod tier;

#[cfg(windows)]
mod adapter;
#[cfg(windows)]
mod descriptors;
#[cfg(windows)]
mod engine;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod features;
#[cfg(windows)]
mod frames;
#[cfg(windows)]
mod monitor;
#[cfg(windows)]
mod swapchain;
#[cfg(windows)]
mod timeline;
#[cfg(windows)]
mod util;

#[cfg(windows)]
pub use adapter::{create_factory, list_adapters, DeviceInfo};
#[cfg(windows)]
pub use descriptors::{CpuHeap, Descriptor, DynamicRange, ShaderVisibleHeap, VisibleDescriptor};
#[cfg(windows)]
pub use engine::{BudgetStatus, Core, DeviceOptions, Recovered};
#[cfg(windows)]
pub use error::Error;
#[cfg(windows)]
pub use features::detect;
#[cfg(windows)]
pub use frames::{CoreFrame, FrameRing, UploadAlloc, UploadRing};
#[cfg(windows)]
pub use monitor::{evict, make_resident, BudgetMonitor, MemorySnapshot};
#[cfg(windows)]
pub use swapchain::{
    BackBuffer, FrameStatistics, PresentMode, PresentStatus, Swapchain, BACK_BUFFER_FORMAT,
};
#[cfg(windows)]
pub use timeline::Timeline;
