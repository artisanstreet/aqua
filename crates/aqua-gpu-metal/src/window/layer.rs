use std::collections::VecDeque;
use std::ptr::NonNull;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use aqua_graph::Extent;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{kCGColorSpaceSRGB, CGColorSpace};
use objc2_metal::{MTLDevice, MTLDrawable, MTLPixelFormat, MTLTexture};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer, CATransaction};

use super::pacing::{FrameWait, InFlight, Screen};
use crate::hal::View;

pub const FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
const DRAWABLES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
    Opaque,
    Premultiplied,
}

#[derive(Default)]
struct Pending {
    in_flight: VecDeque<Extent>,
    shown: Vec<f64>,
    screen: Screen,
}

pub type PresentedWake = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
pub struct Feedback {
    pending: Mutex<Pending>,
    changed: Condvar,
    wake: OnceLock<PresentedWake>,
}

impl Feedback {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn presenting(&self, extent: Extent) {
        self.lock().in_flight.push_back(extent);
    }

    fn presented(&self, media_seconds: f64) {
        let mut pending = self.lock();
        pending.in_flight.pop_front().expect(
            "invariant: a presented handler fires once for each drawable this surface presented",
        );
        pending.shown.push(media_seconds);
        let screen = pending.screen;
        drop(pending);
        self.changed.notify_all();
        match (screen, self.wake.get()) {
            (Screen::Behind | Screen::Unsettled { .. }, Some(wake)) => wake(),
            (Screen::Current, _) | (_, None) => {}
        }
    }

    pub fn in_flight(&self, current: Extent) -> InFlight {
        InFlight::of(self.lock().in_flight.iter().copied(), current)
    }

    pub fn screen(&self) -> Screen {
        self.lock().screen
    }

    pub fn set_screen(&self, screen: Screen) {
        self.lock().screen = screen;
    }

    pub fn wake_with(&self, wake: PresentedWake) -> bool {
        self.wake.set(wake).is_ok()
    }

    pub fn outstanding(&self) -> u32 {
        self.lock().in_flight.len() as u32
    }

    pub fn wait(&self, timeout: Duration) -> FrameWait {
        let pending = self.lock();
        let (pending, _) = self
            .changed
            .wait_timeout_while(pending, timeout, |pending| !pending.in_flight.is_empty())
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match pending.in_flight.is_empty() {
            true => FrameWait::Ready,
            false => FrameWait::TimedOut,
        }
    }

    pub fn take_shown(&self) -> Vec<f64> {
        std::mem::take(&mut self.lock().shown)
    }
}

pub struct Drawable {
    raw: Retained<ProtocolObject<dyn CAMetalDrawable>>,
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
    extent: Extent,
}

impl Drawable {
    pub fn view(&self) -> View<'_> {
        View {
            raw: &self.texture,
            format: FORMAT,
            extent: self.extent,
        }
    }

    pub fn extent(&self) -> Extent {
        self.extent
    }

    pub fn as_drawable(&self) -> &ProtocolObject<dyn MTLDrawable> {
        ProtocolObject::from_ref(&*self.raw)
    }
}

pub struct Layer {
    raw: Retained<CAMetalLayer>,
    extent: Extent,
    sized: Option<Extent>,
    alpha: Alpha,
    feedback: Arc<Feedback>,
}

fn transaction(change: impl FnOnce()) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    change();
    CATransaction::commit();
}

impl Layer {
    pub fn new(
        raw: Retained<CAMetalLayer>,
        device: &ProtocolObject<dyn MTLDevice>,
        extent: Extent,
        alpha: Alpha,
    ) -> Self {
        let colorspace = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
        transaction(|| {
            raw.setDevice(Some(device));
            raw.setPixelFormat(FORMAT);
            raw.setFramebufferOnly(false);
            raw.setMaximumDrawableCount(DRAWABLES);
            raw.setDisplaySyncEnabled(true);
            raw.setAllowsNextDrawableTimeout(true);
            raw.setPresentsWithTransaction(false);
            raw.setWantsExtendedDynamicRangeContent(false);
            raw.setColorspace(colorspace.as_deref());
            raw.setOpaque(alpha == Alpha::Opaque);
        });
        Self {
            raw,
            extent,
            sized: None,
            alpha,
            feedback: Arc::default(),
        }
    }

    pub fn attach(&mut self, device: &ProtocolObject<dyn MTLDevice>) {
        transaction(|| self.raw.setDevice(Some(device)));
    }

    pub fn extent(&self) -> Extent {
        self.extent
    }

    pub fn alpha(&self) -> Alpha {
        self.alpha
    }

    pub fn resize(&mut self, extent: Extent) {
        self.extent = extent;
    }

    pub fn feedback(&self) -> &Feedback {
        &self.feedback
    }

    pub fn bytes(&self) -> u64 {
        self.extent.texels() * 4 * DRAWABLES as u64
    }

    pub fn next_drawable(&mut self) -> Option<Drawable> {
        let extent = self.extent;
        if self.sized != Some(extent) {
            transaction(|| {
                self.raw.setDrawableSize(CGSize {
                    width: f64::from(extent.width),
                    height: f64::from(extent.height),
                })
            });
            self.sized = Some(extent);
        }
        let raw = self.raw.nextDrawable()?;
        let texture = raw.texture();
        let drawn = Extent {
            width: texture.width() as u32,
            height: texture.height() as u32,
        };
        (drawn == extent).then_some(Drawable {
            raw,
            texture,
            extent,
        })
    }

    pub fn watch(&self, drawable: &Drawable) {
        let feedback = Arc::clone(&self.feedback);
        let handler = RcBlock::new(move |shown: NonNull<ProtocolObject<dyn MTLDrawable>>| {
            feedback.presented(unsafe { shown.as_ref() }.presentedTime());
        });
        self.feedback.presenting(drawable.extent());
        unsafe { drawable.raw.addPresentedHandler(RcBlock::as_ptr(&handler)) };
    }
}
