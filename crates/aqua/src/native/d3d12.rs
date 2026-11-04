use std::sync::Once;

use aqua_core::Oklch;
use aqua_gpu_d3d12::window::{TextureRecovery, WindowBackend, WindowOptions};
use aqua_graph::Extent;
use aqua_protocol::{GpuBackend, NativeSurface, SurfaceTarget, WindowId};
use aqua_text::{SharedFonts, SharedRuns};

use crate::render::host::{FrameTarget, GpuFrames};
use crate::render::link::InitFactory;
use crate::render::lower::{Lowering, RecoverySignal};
use crate::render::persist::{self, PipelineCacheSaver};

static TEXT_SERVICES: Once = Once::new();

pub fn factory(
    window: WindowId,
    hwnd: isize,
    extent: Extent,
    fonts: SharedFonts,
    runs: SharedRuns,
    clear: Oklch,
) -> InitFactory {
    Box::new(move |externals, stats| {
        Box::new(move || {
            TEXT_SERVICES.call_once(|| {
                persist::start_text_services(&fonts);
            });
            let cache = persist::load_pipeline_cache();
            let mut backend =
                WindowBackend::with_pipeline_cache(WindowOptions::from_env(), cache.as_deref())
                    .map_err(|e| e.to_string())?;
            let signal = RecoverySignal::default();
            let listener = signal.listener();
            backend.set_recovery_listener(Box::new(move |notice| {
                if notice.textures == TextureRecovery::HostMustReupload {
                    listener.textures_lost();
                }
            }));
            let surface = backend
                .create_surface(SurfaceTarget::Native(NativeSurface {
                    window,
                    hwnd: hwnd as u64,
                    extent,
                }))
                .map_err(|e| e.to_string())?;
            let lowering = Lowering::new(fonts.clone(), runs)
                .with_clear(clear)
                .with_recovery(signal);
            let mut saver = PipelineCacheSaver::default();
            let frames = GpuFrames::new(backend, surface, extent, lowering, externals, stats)
                .with_maintenance(move |backend: &mut WindowBackend| {
                    saver.save(backend.pipeline_cache());
                    persist::save_fallback_index(&fonts);
                });
            Ok(Box::new(frames) as Box<dyn FrameTarget>)
        })
    })
}
