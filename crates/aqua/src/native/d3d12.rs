use aqua_core::Oklch;
use aqua_gpu_d3d12::window::{WindowBackend, WindowOptions};
use aqua_graph::Extent;
use aqua_protocol::{GpuBackend, NativeSurface, SurfaceTarget, WindowId};
use aqua_text::{SharedFonts, SharedRuns};

use crate::render::host::{FrameTarget, GpuFrames};
use crate::render::link::InitFactory;
use crate::render::lower::Lowering;

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
            let mut backend =
                WindowBackend::new(WindowOptions::from_env()).map_err(|e| e.to_string())?;
            let surface = backend
                .create_surface(SurfaceTarget::Native(NativeSurface {
                    window,
                    hwnd: hwnd as u64,
                    extent,
                }))
                .map_err(|e| e.to_string())?;
            let lowering = Lowering::new(fonts, runs).with_clear(clear);
            let frames = GpuFrames::new(backend, surface, extent, lowering, externals, stats);
            Ok(Box::new(frames) as Box<dyn FrameTarget>)
        })
    })
}
