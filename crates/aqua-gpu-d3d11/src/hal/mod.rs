pub mod budget;
pub mod recovery;
pub mod selection;

#[cfg(windows)]
mod adapter;
#[cfg(windows)]
mod engine;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod monitor;
#[cfg(windows)]
mod swapchain;

#[cfg(windows)]
pub use adapter::{DeviceInfo, Selected, create_factory, list_adapters, select as select_adapter};
#[cfg(windows)]
pub use engine::{BudgetStatus, Core, DeviceOptions, Recovered};
#[cfg(windows)]
pub use error::Error;
#[cfg(windows)]
pub use monitor::{BudgetMonitor, MemorySnapshot};
#[cfg(windows)]
pub use swapchain::{PresentMode, PresentStatus, RENDER_FORMAT, Swapchain, SwapchainTarget};
