//! Effect descriptors and composition-pass contracts.
//!
//! Three distinct effects (never conflated):
//! - content filter: element + descendants, isolated group then filter;
//! - in-app backdrop filter: earlier content behind the element;
//! - native window material: platform compositor, NOT our shaders.

use crate::scene::Rect;

/// Separable Gaussian blur parameters. Radius in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurParams {
    pub radius_px: f32,
    /// Downsample factor for large radii (1 = full res). Chosen so the
    /// working radius stays bounded; quality/performance is explicit, not
    /// hidden: downsampling trades high-frequency accuracy for speed and
    /// the factor is visible in scratch allocation sizes.
    pub downsample: u32,
    /// Tap spacing in working-resolution texels. The shader always executes
    /// 9 taps per pass; the step stretches the kernel to cover ±2σ so large
    /// radii don't truncate into a hard cutoff.
    pub step: f32,
}

impl BlurParams {
    pub fn for_radius(radius_px: f32) -> Self {
        let radius_px = radius_px.max(0.0);
        let downsample = ((radius_px / 32.0).ceil() as u32).max(1);
        let sigma_w = radius_px / downsample as f32 / 2.0;
        let step = (sigma_w / 2.0).ceil().max(1.0);
        Self {
            radius_px,
            downsample,
            step,
        }
    }

    /// Working-resolution sigma actually executed.
    pub fn sigma_working(&self) -> f32 {
        (self.radius_px / self.downsample as f32 / 2.0).max(0.5)
    }

    /// Kernel taps actually executed per pixel per pass.
    pub fn taps(&self) -> u32 {
        // 9-tap Gaussian per direction, two passes, adaptive step.
        if self.radius_px < 0.5 {
            1
        } else {
            9
        }
    }

    /// Padded sampling footprint so blur edges don't go dark/sharp.
    /// Covers ±2.5σ (99% of kernel energy) beyond the visible bounds.
    pub fn padding(&self) -> f32 {
        (self.radius_px * 2.5).ceil()
    }
}

/// A custom GPU pass plugged through the public extension contract.
/// Custom passes declare inputs/outputs and participate in the same damage
/// and lifetime rules as built-in passes.
pub trait CustomPass: Send + Sync {
    /// Stable name for diagnostics and inspector.
    fn name(&self) -> &str;
    /// Effect bounds in logical pixels (damage + clip expansion derive here).
    fn bounds(&self) -> Rect;
    /// Whether this pass needs the backdrop (content below it) as input.
    fn reads_backdrop(&self) -> bool;
    /// Encode into `encoder`, reading from `source` and writing to `target`.
    /// Both views are framework-owned and valid for this call only.
    /// Returns `Err` on failure: the renderer aborts the frame (nothing is
    /// submitted, the retained base keeps last good pixels) and propagates
    /// the error to the facade. Failures are never silently ignored.
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(), super::renderer::RenderError>;
}

/// Capabilities the renderer reports honestly. Unsupported entries must
/// return explicit errors, never silent no-ops.
#[derive(Clone, Debug)]
pub struct EffectCapabilities {
    pub rect_fill: bool,
    pub rounded_corners: bool,
    pub border: bool,
    pub drop_shadow: bool,
    pub axis_clip: bool,
    pub rounded_clip: bool,
    pub group_opacity: bool,
    pub backdrop_blur: bool,
    pub content_blur: bool,
    pub custom_passes: bool,
    pub external_layers: bool,
    pub text_runs: bool,
}

impl EffectCapabilities {
    pub fn supported_by_this_renderer() -> Self {
        Self {
            rect_fill: true,
            rounded_corners: true,
            border: true,
            drop_shadow: true,
            axis_clip: true,
            rounded_clip: true,
            group_opacity: true, // isolated intermediate surface, applied once
            backdrop_blur: true,
            content_blur: false, // deferred: needs isolated-group raster pass
            custom_passes: true,
            external_layers: true,
            text_runs: true,
        }
    }

    /// Explicitly unsupported in this implementation (honest ledger).
    pub fn unsupported() -> &'static [&'static str] {
        &[
            "content (self) blur: isolated-group raster pass not yet implemented",
            "blend modes beyond premultiplied-over",
            "arbitrary path masks (rounded-rect clips only)",
            "native window materials (platform adapter responsibility)",
            "HDR / wide-gamut output mapping (linear sRGB working space only)",
            "MSAA (single-sample with shader AA)",
            "displacement / color-matrix filters",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_padding_covers_kernel() {
        let p = BlurParams::for_radius(20.0);
        assert!(p.padding() >= 20.0 * 2.0);
        assert_eq!(p.downsample, 1);
        let big = BlurParams::for_radius(64.0);
        assert_eq!(big.downsample, 2);
        let small = BlurParams::for_radius(8.0);
        assert_eq!(small.downsample, 1);
        // Kernel coverage: 4 taps * step must reach >= 2 working sigma.
        for r in [4.0, 12.0, 32.0, 64.0, 200.0] {
            let q = BlurParams::for_radius(r);
            assert!(
                4.0 * q.step >= 2.0 * q.sigma_working() - 1e-3,
                "r={r}: step={} sigma_w={}",
                q.step,
                q.sigma_working()
            );
        }
    }

    #[test]
    fn capabilities_are_explicit() {
        let caps = EffectCapabilities::supported_by_this_renderer();
        assert!(caps.backdrop_blur && caps.rounded_clip && caps.custom_passes);
        assert!(!caps.content_blur);
        assert!(!EffectCapabilities::unsupported().is_empty());
    }
}
