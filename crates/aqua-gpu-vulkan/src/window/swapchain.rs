use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use aqua_graph::Extent;
use aqua_protocol::WaylandSurface;
use ash::vk;

use crate::hal::{Access, Checked, Device, Error, ImageRef, Presenting, COLOR};
use crate::ledger::{record, Kind, Token};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alpha {
    #[default]
    Opaque,
    Premultiplied,
}

#[derive(Debug, thiserror::Error)]
pub enum SwapchainError {
    #[error(transparent)]
    Device(#[from] Error),
    #[error("the device was not created for presenting")]
    NotPresenting,
    #[error("queue family {family} cannot present to this Wayland surface")]
    NoPresentQueue { family: u32 },
    #[error("the Wayland surface offers neither B8G8R8A8_UNORM nor R8G8B8A8_UNORM")]
    NoFormat,
    #[error("the Wayland surface does not support {alpha:?} composite alpha")]
    NoAlpha { alpha: Alpha },
    #[error("the Wayland surface cannot be rendered to: usage {supported:?}")]
    NoUsage { supported: vk::ImageUsageFlags },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    Free,
    Busy,
    Submitted(u64),
    Presented(u32),
}

struct Semaphores {
    entries: Vec<(vk::Semaphore, Hold)>,
}

impl Semaphores {
    fn take(&mut self, device: &Device) -> Result<vk::Semaphore, Error> {
        let completed = device.completed_serial();
        let reusable = self.entries.iter_mut().find(|(_, hold)| match hold {
            Hold::Free => true,
            Hold::Submitted(serial) => *serial <= completed,
            Hold::Busy | Hold::Presented(_) => false,
        });
        if let Some((semaphore, hold)) = reusable {
            *hold = Hold::Busy;
            return Ok(*semaphore);
        }
        let semaphore = device.observe(
            unsafe {
                device
                    .raw
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .checked("vkCreateSemaphore"),
        )?;
        self.entries.push((semaphore, Hold::Busy));
        Ok(semaphore)
    }

    fn set(&mut self, semaphore: vk::Semaphore, hold: Hold) {
        let entry = self
            .entries
            .iter_mut()
            .find(|(candidate, _)| *candidate == semaphore)
            .expect("a swapchain only tracks semaphores from its own pool");
        entry.1 = hold;
    }

    fn reacquired(&mut self, image: u32) {
        self.entries
            .iter_mut()
            .filter(|(_, hold)| *hold == Hold::Presented(image))
            .for_each(|(_, hold)| *hold = Hold::Free);
    }

    fn settle(&mut self) {
        self.entries
            .iter_mut()
            .for_each(|(_, hold)| *hold = Hold::Free);
    }
}

struct SwapImage {
    image: vk::Image,
    view: vk::ImageView,
    access: Cell<Access>,
}

struct Chain {
    raw: vk::SwapchainKHR,
    images: Vec<SwapImage>,
    extent: Extent,
    _token: Token,
}

pub enum Acquire {
    Image {
        index: u32,
        semaphore: vk::Semaphore,
    },
    Unavailable,
    Stale,
}

pub enum PresentOutcome {
    Presented,
    Stale,
}

pub struct Swapchain {
    device: Arc<Device>,
    surface: vk::SurfaceKHR,
    format: vk::Format,
    alpha: Alpha,
    mode: vk::PresentModeKHR,
    chain: Chain,
    semaphores: Semaphores,
}

fn presenting(device: &Device) -> Result<Presenting<'_>, SwapchainError> {
    device.presenting().ok_or(SwapchainError::NotPresenting)
}

fn composite(alpha: Alpha) -> vk::CompositeAlphaFlagsKHR {
    match alpha {
        Alpha::Opaque => vk::CompositeAlphaFlagsKHR::OPAQUE,
        Alpha::Premultiplied => vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
    }
}

const WANTED_USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
    vk::ImageUsageFlags::COLOR_ATTACHMENT.as_raw()
        | vk::ImageUsageFlags::TRANSFER_SRC.as_raw()
        | vk::ImageUsageFlags::TRANSFER_DST.as_raw()
        | vk::ImageUsageFlags::SAMPLED.as_raw(),
);

const REQUIRED_USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
    vk::ImageUsageFlags::COLOR_ATTACHMENT.as_raw() | vk::ImageUsageFlags::TRANSFER_SRC.as_raw(),
);

fn choose_mode(modes: &[vk::PresentModeKHR]) -> vk::PresentModeKHR {
    [vk::PresentModeKHR::MAILBOX, vk::PresentModeKHR::IMMEDIATE]
        .into_iter()
        .find(|mode| modes.contains(mode))
        .unwrap_or(vk::PresentModeKHR::FIFO)
}

fn choose_format(formats: &[vk::SurfaceFormatKHR]) -> Result<vk::Format, SwapchainError> {
    [vk::Format::B8G8R8A8_UNORM, vk::Format::R8G8B8A8_UNORM]
        .into_iter()
        .find(|wanted| {
            formats.iter().any(|offered| {
                offered.format == *wanted
                    && offered.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
        })
        .ok_or(SwapchainError::NoFormat)
}

fn create_surface(
    device: &Device,
    target: &WaylandSurface,
) -> Result<vk::SurfaceKHR, SwapchainError> {
    let presenting = presenting(device)?;
    let info = vk::WaylandSurfaceCreateInfoKHR::default()
        .display(target.display as *mut vk::wl_display)
        .surface(target.surface as *mut vk::wl_surface);
    let surface = device.observe(
        unsafe {
            presenting
                .loaders
                .wayland
                .create_wayland_surface(&info, None)
        }
        .checked("vkCreateWaylandSurfaceKHR"),
    )?;
    let supported = unsafe {
        presenting
            .loaders
            .surface
            .get_physical_device_surface_support(presenting.physical, presenting.family, surface)
    }
    .checked("vkGetPhysicalDeviceSurfaceSupportKHR");
    match supported {
        Ok(true) => Ok(surface),
        outcome => {
            unsafe { presenting.loaders.surface.destroy_surface(surface, None) };
            match outcome {
                Err(error) => Err(error.into()),
                Ok(_) => Err(SwapchainError::NoPresentQueue {
                    family: presenting.family,
                }),
            }
        }
    }
}

fn clamp(wanted: u32, low: u32, high: u32) -> u32 {
    wanted.clamp(low.max(1), high.max(low.max(1)))
}

impl Swapchain {
    pub fn new(
        device: &Arc<Device>,
        target: &WaylandSurface,
        alpha: Alpha,
    ) -> Result<Self, SwapchainError> {
        let surface = create_surface(device, target)?;
        let built = Self::configure(device, surface, alpha).and_then(|(format, mode)| {
            let chain = Self::build(
                device,
                surface,
                format,
                alpha,
                mode,
                target.extent,
                vk::SwapchainKHR::null(),
            )?;
            Ok((format, mode, chain))
        });
        match built {
            Ok((format, mode, chain)) => Ok(Self {
                device: Arc::clone(device),
                surface,
                format,
                alpha,
                mode,
                chain,
                semaphores: Semaphores {
                    entries: Vec::new(),
                },
            }),
            Err(error) => {
                if let Some(presenting) = device.presenting() {
                    unsafe { presenting.loaders.surface.destroy_surface(surface, None) };
                }
                Err(error)
            }
        }
    }

    fn configure(
        device: &Device,
        surface: vk::SurfaceKHR,
        alpha: Alpha,
    ) -> Result<(vk::Format, vk::PresentModeKHR), SwapchainError> {
        let presenting = presenting(device)?;
        let loader = &presenting.loaders.surface;
        let physical = presenting.physical;
        let formats = unsafe { loader.get_physical_device_surface_formats(physical, surface) }
            .checked("vkGetPhysicalDeviceSurfaceFormatsKHR")?;
        let modes = unsafe { loader.get_physical_device_surface_present_modes(physical, surface) }
            .checked("vkGetPhysicalDeviceSurfacePresentModesKHR")?;
        let capabilities =
            unsafe { loader.get_physical_device_surface_capabilities(physical, surface) }
                .checked("vkGetPhysicalDeviceSurfaceCapabilitiesKHR")?;
        if !capabilities
            .supported_composite_alpha
            .contains(composite(alpha))
        {
            return Err(SwapchainError::NoAlpha { alpha });
        }
        if !capabilities.supported_usage_flags.contains(REQUIRED_USAGE) {
            return Err(SwapchainError::NoUsage {
                supported: capabilities.supported_usage_flags,
            });
        }
        Ok((choose_format(&formats)?, choose_mode(&modes)))
    }

    fn build(
        device: &Arc<Device>,
        surface: vk::SurfaceKHR,
        format: vk::Format,
        alpha: Alpha,
        mode: vk::PresentModeKHR,
        extent: Extent,
        old: vk::SwapchainKHR,
    ) -> Result<Chain, SwapchainError> {
        let presenting = presenting(device)?;
        let capabilities = unsafe {
            presenting
                .loaders
                .surface
                .get_physical_device_surface_capabilities(presenting.physical, surface)
        }
        .checked("vkGetPhysicalDeviceSurfaceCapabilitiesKHR")?;
        let high = match capabilities.max_image_count {
            0 => u32::MAX,
            count => count,
        };
        let count = clamp(
            capabilities.min_image_count.max(3),
            capabilities.min_image_count,
            high,
        );
        let size = vk::Extent2D {
            width: clamp(
                extent.width,
                capabilities.min_image_extent.width,
                capabilities.max_image_extent.width,
            ),
            height: clamp(
                extent.height,
                capabilities.min_image_extent.height,
                capabilities.max_image_extent.height,
            ),
        };
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(count)
            .image_format(format)
            .image_color_space(vk::ColorSpaceKHR::SRGB_NONLINEAR)
            .image_extent(size)
            .image_array_layers(1)
            .image_usage(WANTED_USAGE & capabilities.supported_usage_flags)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
            .composite_alpha(composite(alpha))
            .present_mode(mode)
            .clipped(true)
            .old_swapchain(old);
        let loader = presenting.swapchain;
        let raw = device.observe(
            unsafe { loader.create_swapchain(&info, None) }.checked("vkCreateSwapchainKHR"),
        )?;
        let images = device.observe(
            unsafe { loader.get_swapchain_images(raw) }
                .checked("vkGetSwapchainImagesKHR")
                .and_then(|images| Self::views(device, format, &images)),
        );
        let images = match images {
            Ok(images) => images,
            Err(error) => {
                unsafe { loader.destroy_swapchain(raw, None) };
                return Err(error.into());
            }
        };
        let bytes = u64::from(size.width) * u64::from(size.height) * 4 * images.len() as u64;
        Ok(Chain {
            raw,
            images,
            extent: Extent {
                width: size.width,
                height: size.height,
            },
            _token: record("swapchain", Kind::Swapchain, bytes),
        })
    }

    fn views(
        device: &Device,
        format: vk::Format,
        images: &[vk::Image],
    ) -> Result<Vec<SwapImage>, Error> {
        let mut built = Vec::with_capacity(images.len());
        for image in images {
            let info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(COLOR);
            match unsafe { device.raw.create_image_view(&info, None) }.checked("vkCreateImageView")
            {
                Ok(view) => built.push(SwapImage {
                    image: *image,
                    view,
                    access: Cell::new(Access::Acquired),
                }),
                Err(error) => {
                    built.iter().for_each(|image| unsafe {
                        device.raw.destroy_image_view(image.view, None)
                    });
                    return Err(error);
                }
            }
        }
        Ok(built)
    }

    pub fn extent(&self) -> Extent {
        self.chain.extent
    }

    pub fn format(&self) -> vk::Format {
        self.format
    }

    pub fn alpha(&self) -> Alpha {
        self.alpha
    }

    pub fn present_mode(&self) -> vk::PresentModeKHR {
        self.mode
    }

    pub fn image_count(&self) -> usize {
        self.chain.images.len()
    }

    pub fn bytes(&self) -> u64 {
        u64::from(self.chain.extent.width)
            * u64::from(self.chain.extent.height)
            * 4
            * self.chain.images.len() as u64
    }

    pub fn image(&self, index: u32) -> ImageRef<'_> {
        let image = &self.chain.images[index as usize];
        ImageRef {
            image: image.image,
            view: image.view,
            format: self.format,
            extent: self.chain.extent,
            access: &image.access,
        }
    }

    pub fn acquire(&mut self, timeout: Duration) -> Result<Acquire, Error> {
        let semaphore = self.semaphores.take(&self.device)?;
        let presenting = self
            .device
            .presenting()
            .expect("a swapchain only exists on a presenting device");
        let nanos = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
        let acquired = unsafe {
            presenting.swapchain.acquire_next_image(
                self.chain.raw,
                nanos,
                semaphore,
                vk::Fence::null(),
            )
        };
        match acquired {
            Ok((index, _)) => {
                self.semaphores.reacquired(index);
                Ok(Acquire::Image { index, semaphore })
            }
            Err(vk::Result::TIMEOUT | vk::Result::NOT_READY) => {
                self.semaphores.set(semaphore, Hold::Free);
                Ok(Acquire::Unavailable)
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.semaphores.set(semaphore, Hold::Free);
                Ok(Acquire::Stale)
            }
            Err(result) => {
                self.semaphores.set(semaphore, Hold::Free);
                self.device
                    .observe(Err::<(), _>(result).checked("vkAcquireNextImageKHR"))?;
                Ok(Acquire::Unavailable)
            }
        }
    }

    pub fn signal_semaphore(&mut self) -> Result<vk::Semaphore, Error> {
        self.semaphores.take(&self.device)
    }

    pub fn submitted(&mut self, semaphore: vk::Semaphore, serial: u64) {
        self.semaphores.set(semaphore, Hold::Submitted(serial));
    }

    pub fn present(
        &mut self,
        index: u32,
        rendered: vk::Semaphore,
        damage: &[vk::RectLayerKHR],
    ) -> Result<PresentOutcome, Error> {
        let presenting = self
            .device
            .presenting()
            .expect("a swapchain only exists on a presenting device");
        let waits = [rendered];
        let swapchains = [self.chain.raw];
        let indices = [index];
        let regions = [vk::PresentRegionKHR::default().rectangles(damage)];
        let mut incremental = vk::PresentRegionsKHR::default().regions(&regions);
        let mut info = vk::PresentInfoKHR::default()
            .wait_semaphores(&waits)
            .swapchains(&swapchains)
            .image_indices(&indices);
        if presenting.incremental && !damage.is_empty() {
            info = info.push_next(&mut incremental);
        }
        let presented = unsafe { presenting.swapchain.queue_present(self.device.queue, &info) };
        self.semaphores.set(rendered, Hold::Presented(index));
        match presented {
            Ok(_) => Ok(PresentOutcome::Presented),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(PresentOutcome::Stale),
            Err(result) => {
                self.device
                    .observe(Err::<(), _>(result).checked("vkQueuePresentKHR"))?;
                Ok(PresentOutcome::Stale)
            }
        }
    }

    pub fn release(&mut self, semaphore: vk::Semaphore) -> Result<(), Error> {
        let waited = self.device.wait_semaphore(semaphore);
        self.semaphores.set(semaphore, Hold::Submitted(u64::MAX));
        waited
    }

    pub fn resize(&mut self, extent: Extent) -> Result<(), SwapchainError> {
        self.device.idle()?;
        let chain = Self::build(
            &self.device,
            self.surface,
            self.format,
            self.alpha,
            self.mode,
            extent,
            self.chain.raw,
        )?;
        let mut old = std::mem::replace(&mut self.chain, chain);
        Self::destroy_chain(&self.device, &mut old);
        self.semaphores.settle();
        Ok(())
    }

    fn destroy_chain(device: &Device, chain: &mut Chain) {
        let presenting = device
            .presenting()
            .expect("a swapchain only exists on a presenting device");
        chain
            .images
            .drain(..)
            .for_each(|image| unsafe { device.raw.destroy_image_view(image.view, None) });
        unsafe {
            presenting
                .swapchain
                .destroy_swapchain(std::mem::take(&mut chain.raw), None)
        };
    }
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        let _ = self.device.idle();
        Self::destroy_chain(&self.device, &mut self.chain);
        self.semaphores
            .entries
            .drain(..)
            .for_each(|(semaphore, _)| unsafe {
                self.device.raw.destroy_semaphore(semaphore, None)
            });
        if let Some(presenting) = self.device.presenting() {
            unsafe {
                presenting
                    .loaders
                    .surface
                    .destroy_surface(self.surface, None)
            };
        }
    }
}
