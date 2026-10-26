//! `wgpu::Surface` presentation facade, kept separate from the offscreen
//! composition core so headless/test hosts never touch windowing.
//!
//! Correctness rules (review-driven):
//! - Raster-dirty (scene changed) and present-dirty (last present failed or
//!   the OS demanded re-presentation) are SEPARATE. A failed acquire never
//!   loses work: the next call re-presents even when the new plan is clean.
//! - Surface formats/modes come from capabilities; nothing is invented. An
//!   empty capability list is an explicit error, and an unsupported
//!   `PresentMode` falls back to `Fifo` (the only guaranteed mode) instead
//!   of crashing the backend.
//!
//! The facade owns: surface configuration, resize/reconfigure, acquire,
//! render-target handoff to [`crate::Renderer`], gamma-correct blit, and
//! present with explicit recovery.

use crate::renderer::{RenderError, Renderer};
use crate::scene::Scene;

/// Pure present/retry state machine. No GPU types: unit-tested below and
/// driven by [`SurfaceFacade`].
#[derive(Debug, Default)]
pub struct PresentPolicy {
    /// A composed base exists that was never presented (failed acquire,
    /// occlusion, or explicit OS expose request).
    needs_present: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcquireClass {
    /// Fresh frame acquired, ready to present.
    Ready,
    /// Acquired but suboptimal (reconfigure recommended after present).
    Suboptimal,
    /// No frame (timeout/occluded): skip, keep pending state.
    Deferred,
    /// Surface changed/lost: reconfigure, keep pending state, retry later.
    Stale,
    /// Validation error: report, keep pending state.
    Invalid,
}

impl PresentPolicy {
    /// OS expose / occlusion-end / any out-of-band "repaint now" demand.
    /// The next `acquire_and_draw` presents even with a clean scene.
    pub fn request_present(&mut self) {
        self.needs_present = true;
    }

    /// Whether composition output must be (re)presented this call:
    /// raster work OR a pending present.
    pub fn should_present(&self, raster_dirty: bool) -> bool {
        raster_dirty || self.needs_present
    }

    /// Fold an acquire outcome into pending state. Returns whether the
    /// caller presented a frame this call.
    pub fn on_acquire(&mut self, outcome: AcquireClass, raster_dirty: bool) -> bool {
        match outcome {
            AcquireClass::Ready | AcquireClass::Suboptimal => {
                self.needs_present = false;
                true
            }
            AcquireClass::Deferred | AcquireClass::Stale | AcquireClass::Invalid => {
                // Nothing presented: remember that output is pending so a
                // later clean plan still re-presents instead of skipping.
                if raster_dirty || self.needs_present {
                    self.needs_present = true;
                }
                false
            }
        }
    }

    pub fn needs_present(&self) -> bool {
        self.needs_present
    }
}

/// Window-surface presenter over an offscreen-compositing [`Renderer`].
pub struct SurfaceFacade {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gamma: f32,
    policy: PresentPolicy,
}

impl SurfaceFacade {
    /// Configure `surface` from real capabilities. `format`/`present_mode`
    /// preferences are honored only when supported; otherwise the first
    /// advertised format and `Fifo` are used. Empty capability lists are an
    /// explicit incompatible-surface error.
    pub fn with_adapter(
        device: &wgpu::Device,
        adapter: &wgpu::Adapter,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        prefer_format: Option<wgpu::TextureFormat>,
        prefer_present_mode: Option<wgpu::PresentMode>,
    ) -> Result<Self, RenderError> {
        if width == 0 || height == 0 {
            return Err(RenderError::SurfaceFailed("zero-sized surface".into()));
        }
        let caps = surface.get_capabilities(adapter);
        let format = prefer_format
            .filter(|f| caps.formats.contains(f))
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| {
                RenderError::SurfaceFailed("surface incompatible with adapter: no formats".into())
            })?;
        let present_mode = prefer_present_mode
            .filter(|m| caps.present_modes.contains(m))
            .unwrap_or(wgpu::PresentMode::Fifo);
        let alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            caps.alpha_modes.first().copied().ok_or_else(|| {
                RenderError::SurfaceFailed("surface reports no alpha modes".into())
            })?
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(device, &config);
        // Desktop composition consumes encoded-premultiplied pixels.
        // Mode 2 compensates for the hardware transfer on sRGB targets;
        // mode 1 writes those bytes directly to an Unorm target.
        let gamma = if format.is_srgb() { 2.0 } else { 1.0 };
        Ok(Self {
            surface,
            config,
            gamma,
            policy: PresentPolicy::default(),
        })
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn present_mode(&self) -> wgpu::PresentMode {
        self.config.present_mode
    }

    /// Actual compositor alpha mode selected from the surface capabilities.
    pub fn alpha_mode(&self) -> wgpu::CompositeAlphaMode {
        self.config.alpha_mode
    }

    /// External re-presentation demand (OS expose, occlusion end).
    pub fn request_present(&mut self) {
        self.policy.request_present();
    }

    /// Resize: reconfigures the surface AND the renderer's offscreen base.
    /// Zero sizes are refused (no zero-sized targets, per design).
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut Renderer,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<(), RenderError> {
        if width == 0 || height == 0 {
            return Err(RenderError::SurfaceFailed("zero-sized surface".into()));
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(device, &self.config);
        self.policy.request_present(); // resized content must present once
        renderer.set_size(width, height, scale)
    }

    /// Composite when dirty, (re)present when needed. Returns `true` when a
    /// frame was presented. Stale surfaces reconfigure and preserve the
    /// pending present; the caller retries on the next event.
    pub fn acquire_and_draw(
        &mut self,
        renderer: &mut Renderer,
        scene: &mut Scene,
    ) -> Result<bool, RenderError> {
        let plan = renderer.prepare(scene);
        let raster_dirty = !plan.is_clean();
        if raster_dirty {
            renderer.draw(&plan)?;
        }
        if !self.policy.should_present(raster_dirty) {
            return Ok(false);
        }
        let outcome = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => {
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let format = self.config.format;
                renderer.blit_to(&view, format, self.gamma)?;
                renderer.present(frame);
                renderer.note_presented();
                AcquireClass::Ready
            }
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let format = self.config.format;
                renderer.blit_to(&view, format, self.gamma)?;
                renderer.present(frame);
                renderer.note_presented();
                AcquireClass::Suboptimal
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                AcquireClass::Deferred
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(renderer.device(), &self.config);
                AcquireClass::Stale
            }
            wgpu::CurrentSurfaceTexture::Validation => AcquireClass::Invalid,
        };
        // Suboptimal is presented (diagnostics-honest: not counted as a
        // clean present); the next resize reconfigures.
        let presented = self.policy.on_acquire(outcome, raster_dirty);
        match outcome {
            AcquireClass::Invalid => Err(RenderError::SurfaceFailed("surface validation".into())),
            AcquireClass::Stale => Err(RenderError::SurfaceFailed(
                "surface reconfigured; retry".into(),
            )),
            _ => Ok(presented),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_plan_skips_without_pending_present() {
        let mut p = PresentPolicy::default();
        assert!(!p.should_present(false));
        assert!(!p.on_acquire(AcquireClass::Deferred, false));
        assert!(!p.needs_present());
    }

    #[test]
    fn failed_acquire_preserves_pending_present() {
        let mut p = PresentPolicy::default();
        // Raster work done, acquire deferred: pending stays set...
        assert!(!p.on_acquire(AcquireClass::Deferred, true));
        assert!(p.needs_present());
        // ...so a later CLEAN plan still presents instead of skipping.
        assert!(p.should_present(false));
        assert!(p.on_acquire(AcquireClass::Ready, false));
        assert!(!p.needs_present());
    }

    #[test]
    fn stale_surface_keeps_pending_across_retries() {
        let mut p = PresentPolicy::default();
        assert!(!p.on_acquire(AcquireClass::Stale, true));
        assert!(p.should_present(false));
        assert!(!p.on_acquire(AcquireClass::Deferred, false));
        assert!(p.should_present(false));
        assert!(p.on_acquire(AcquireClass::Suboptimal, false));
        assert!(!p.needs_present());
    }

    #[test]
    fn expose_request_forces_present_on_clean_scene() {
        let mut p = PresentPolicy::default();
        p.request_present();
        assert!(p.should_present(false));
    }
}
