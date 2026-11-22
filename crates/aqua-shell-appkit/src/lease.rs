use aqua_protocol::WindowId;
use objc2::rc::Retained;
use objc2_quartz_core::CAMetalLayer;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerGeometry {
    pub logical: (f64, f64),
    pub physical: (u32, u32),
    pub scale: f64,
    pub refresh: Option<Duration>,
    pub covered: bool,
}

#[derive(Clone)]
pub struct LayerLease {
    window: WindowId,
    layer: Retained<CAMetalLayer>,
    geometry: Arc<Mutex<LayerGeometry>>,
}

unsafe impl Send for LayerLease {}
unsafe impl Sync for LayerLease {}

impl LayerLease {
    pub(crate) fn new(
        window: WindowId,
        layer: Retained<CAMetalLayer>,
        geometry: Arc<Mutex<LayerGeometry>>,
    ) -> Self {
        Self {
            window,
            layer,
            geometry,
        }
    }

    pub fn window(&self) -> WindowId {
        self.window
    }

    pub fn layer(&self) -> Retained<CAMetalLayer> {
        self.layer.clone()
    }

    pub fn geometry(&self) -> LayerGeometry {
        *self
            .geometry
            .lock()
            .expect("invariant: the layer geometry lock is never poisoned")
    }
}

pub(crate) fn update(geometry: &Mutex<LayerGeometry>, change: impl FnOnce(&mut LayerGeometry)) {
    change(
        &mut geometry
            .lock()
            .expect("invariant: the layer geometry lock is never poisoned"),
    );
}
