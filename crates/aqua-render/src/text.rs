//! Text rasterization through the agreed backend (glyphon 0.12 /
//! cosmic-text 0.19) against the SHARED font database.
//!
//! Ownership (text agent): this file owns provider construction, run
//! resolution, and same-encoder drawing. The renderer owns the call sites
//! (`renderer.rs` op walk, `surface.rs`): it queues [`ShapedRun`]s in
//! painter order and hands over its open [`wgpu::CommandEncoder`].
//!
//! Rules enforced here, not by convention:
//!
//! * NO second `FontSystem`. The backend holds [`SharedFonts`] /
//!   [`SharedRuns`] clones. Glyphon's `prepare` borrows `&mut FontSystem`
//!   from the caller, so measurement and pixels share one shaping.
//! * NO debug-fallback strings. Unknown run ids are skipped and counted
//!   ([`TextStats::stale_skipped`]).
//! * STRICT revision coherence: a scene revision older than the registry
//!   revision is SKIPPED and counted ([`TextStats::revision_mismatches`]) —
//!   the current layout is NEVER painted under a stale `TextRun` revision,
//!   since old scene geometry/state plus new text would be an incoherent
//!   commit. The host must repair by re-committing the scene run with the
//!   live revision (UI writes the registry BEFORE the scene, so the next
//!   consistent commit paints).
//! * One `prepare` per `encode_queued` call, immediately followed by its
//!   `render` into the SUPPLIED encoder. Never prepare twice without an
//!   intervening render: glyphon's internal instance buffer would be
//!   overwritten and the first segment would draw the second's glyphs.
//! * Atlas trim happens only after the frame submits, via
//!   [`TextProvider::trim_atlas`] (renderer calls it at the end of `draw`).
//! * Telemetry is honest: glyphon 0.12 exposes no upload-byte counter, so
//!   [`TextStats::atlas_upload_bytes`] is `None` (unknown) — never a
//!   fabricated `areas * 256` guess, which was not even a lower bound
//!   (fully cached glyphs upload zero bytes).
//!
//! Clips: each run carries its painter-order clip in physical px, mapped to
//! `glyphon::TextBounds`. Group alpha is correct by construction: group
//! texts draw into the isolated scratch surface with FULL color and the
//! group opacity applies once at composite; top-level text alpha is
//! premultiplied by the item opacity at resolve time.

use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_text::{ResolveError, RunHandle, RunId, RunRegistry, SharedFonts, SharedRuns};

use glyphon::{
    Cache, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};

/// Backend handles shared with the UI side: one font database and one run
/// registry for the whole process. Construct once (facade) and hand clones
/// to the renderer. No second font database is ever created here.
#[derive(Clone)]
pub struct TextBackend {
    pub fonts: SharedFonts,
    pub runs: SharedRuns,
}

impl TextBackend {
    pub fn new(fonts: SharedFonts, runs: SharedRuns) -> Self {
        Self { fonts, runs }
    }

    /// Convenience for tests and single-owner hosts: fresh system fonts plus
    /// an empty run registry.
    pub fn with_system_fonts() -> Self {
        Self {
            fonts: SharedFonts::new(),
            runs: SharedRuns::new(),
        }
    }
}

/// One text item queued in painter order. The string lives in the shared
/// [`RunRegistry`](aqua_text::RunRegistry); this struct carries only the
/// placement data the renderer owns. `revision` is the scene-committed
/// revision; mismatches resolve to the current layout and are counted.
#[derive(Clone, Debug)]
pub struct ShapedRun {
    /// `TextRun.id` from the scene.
    pub id: u64,
    /// `TextRun.revision` from the scene.
    pub revision: u64,
    /// Top-left origin in physical px.
    pub at_physical: [f32; 2],
    /// Glyph color, sRGB-ish u8 (see [`resolve_scene_text`]), alpha ALREADY
    /// multiplied by the item opacity for top-level text. Group text keeps
    /// full alpha: the group composite applies opacity once.
    pub color_glyphon: glyphon::Color,
    /// Painter-order clip in physical px. `None` = unclipped.
    pub clip_physical: Option<[f32; 4]>,
}

/// Per-frame context for [`TextProvider::encode_queued`]. The encoder stays
/// open and owned by the renderer; text appends ONE render pass (`Load`)
/// in painter order. No submit happens here.
pub struct TextFrame<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub target: &'a wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

/// Honest per-call statistics. Everything here is an exact count except
/// `atlas_upload_bytes`, which is `None` because glyphon exposes no upload
/// counter. Callers must NOT add estimates into real byte totals.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextStats {
    /// Areas resolved and prepared.
    pub areas: usize,
    /// Runs skipped for unknown ids (removed/unmounted text).
    pub stale_skipped: u64,
    /// Runs SKIPPED because the scene revision is older than the registry
    /// revision. Nothing is painted for these: painting the current layout
    /// under a stale revision would mix old scene state with new text.
    /// The host must repair by re-committing the scene run at the live
    /// revision; treat a nonzero count as a re-render request.
    pub revision_mismatches: u64,
    /// Atlas upload bytes. Always `None`: unknown, not zero.
    pub atlas_upload_bytes: Option<u64>,
}

pub trait TextProvider: Send + Sync {
    fn queue_run(&mut self, run: ShapedRun);
    /// Resolve queued runs against the shared registry, prepare ONCE for
    /// all of them, and encode ONE `Load` render pass into the supplied
    /// encoder in painter order. Returns exact counts (see [`TextStats`]).
    fn encode_queued(&mut self, frame: TextFrame<'_>) -> TextStats;
    /// Measured logical ink rect at the origin for damage footprints.
    /// `None` = unknown run (caller must invalidate conservatively).
    fn measure_ink(&mut self, id: u64) -> Option<Rect>;
    /// Atlas trim. Call ONCE after the frame's final submit, never between
    /// encode and submit.
    fn trim_atlas(&mut self);
    fn stats_last(&self) -> TextStats;
}

/// Why a queued run produces no area. The strict coherence policy lives
/// here so the encoder and the tests share one decision point: a stale
/// scene revision yields NO handle — only the live revision's `u64` — so
/// the current (different) string is unreachable and can never be painted
/// under the old revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SkipReason {
    Unknown,
    StaleRevision { current: u64 },
}

fn resolve_for_encode<'a>(
    runs: &'a RunRegistry,
    run: &ShapedRun,
) -> Result<RunHandle<'a>, SkipReason> {
    match runs.resolve_expected(RunId(run.id), run.revision) {
        Ok(h) => Ok(h),
        Err(ResolveError::RevisionMismatch { current }) => {
            Err(SkipReason::StaleRevision { current })
        }
        Err(ResolveError::Unknown) => Err(SkipReason::Unknown),
    }
}

/// Glyphon-backed provider over the shared backend.
pub struct GlyphonProvider {
    backend: TextBackend,
    swash_cache: SwashCache,
    cache: Cache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: TextRenderer,
    pending: Vec<ShapedRun>,
    last_stats: TextStats,
}

impl GlyphonProvider {
    /// Build for `device`/`format` against the SHARED backend. Loads no
    /// fonts: system/custom fonts come from `backend.fonts`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        backend: TextBackend,
    ) -> Self {
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let viewport = Viewport::new(device, &cache);
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        Self {
            backend,
            swash_cache,
            cache,
            atlas,
            viewport,
            renderer,
            pending: Vec::new(),
            last_stats: TextStats::default(),
        }
    }

    pub fn backend(&self) -> &TextBackend {
        &self.backend
    }

    pub fn set_format(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) {
        self.cache = Cache::new(device);
        self.atlas = TextAtlas::new(device, queue, &self.cache, format);
        self.viewport = Viewport::new(device, &self.cache);
        self.renderer = TextRenderer::new(
            &mut self.atlas,
            device,
            wgpu::MultisampleState::default(),
            None,
        );
    }
}

impl TextProvider for GlyphonProvider {
    fn queue_run(&mut self, run: ShapedRun) {
        self.pending.push(run);
    }

    fn encode_queued(&mut self, frame: TextFrame<'_>) -> TextStats {
        let runs = std::mem::take(&mut self.pending);
        let mut stats = TextStats::default();
        if runs.is_empty() {
            self.last_stats = stats;
            return stats;
        }
        self.viewport.update(
            frame.queue,
            Resolution {
                width: frame.width,
                height: frame.height,
            },
        );
        // Locks held through prepare AND render: the registry/buffers must
        // not move while glyphon reads them. Short critical section, no
        // user code runs under it.
        let mut fonts = self.backend.fonts.lock();
        let runs_guard = self.backend.runs.lock();
        let areas: Vec<TextArea> = runs
            .iter()
            .filter_map(|run| {
                let handle = match resolve_for_encode(&runs_guard, run) {
                    Ok(h) => h,
                    Err(SkipReason::StaleRevision { .. }) => {
                        stats.revision_mismatches += 1;
                        return None;
                    }
                    Err(SkipReason::Unknown) => {
                        stats.stale_skipped += 1;
                        return None;
                    }
                };
                let Some(buffer) = handle.layout.buffer() else {
                    stats.stale_skipped += 1;
                    return None;
                };
                Some(TextArea {
                    buffer,
                    left: run.at_physical[0],
                    top: run.at_physical[1],
                    scale: 1.0,
                    bounds: clip_to_bounds(run.clip_physical),
                    default_color: run.color_glyphon,
                    custom_glyphs: &[],
                })
            })
            .collect();
        stats.areas = areas.len();
        if areas.is_empty() {
            self.last_stats = stats;
            return stats;
        }
        // Single prepare for ALL areas of this target, then its render.
        // This pairing is the invariant: never prepare(A)+prepare(B) before
        // render(A), or A's draw reads B's instance buffer.
        if let Err(e) = self.renderer.prepare(
            frame.device,
            frame.queue,
            fonts.font_system_mut(),
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        ) {
            eprintln!("aqua-render: glyphon prepare failed: {e:?}");
            self.last_stats = stats;
            return stats;
        }
        {
            let mut pass = frame
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("aqua-text-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: frame.target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            if let Err(e) = self.renderer.render(&self.atlas, &self.viewport, &mut pass) {
                eprintln!("aqua-render: glyphon render failed: {e:?}");
            }
        }
        // atlas_upload_bytes stays None: glyphon exposes no counter and a
        // guess (old code: areas*256) is fabrication, not even a lower
        // bound — fully cached glyphs upload zero bytes.
        self.last_stats = stats;
        stats
    }

    fn measure_ink(&mut self, id: u64) -> Option<Rect> {
        let guard = self.backend.runs.lock();
        let handle = guard.resolve(RunId(id))?;
        let (w, h) = handle.layout.last_scaled_size()?;
        let s = handle.layout.scale().max(0.01);
        Some(Rect {
            x: 0.0,
            y: 0.0,
            w: (w / s).max(1.0),
            h: (h / s).max(1.0),
        })
    }

    fn trim_atlas(&mut self) {
        self.atlas.trim();
    }

    fn stats_last(&self) -> TextStats {
        self.last_stats
    }
}

fn clip_to_bounds(clip: Option<[f32; 4]>) -> TextBounds {
    match clip {
        // Negative/zero-size clips clip everything; clamp to an empty band.
        Some([x, y, w, h]) if w > 0.0 && h > 0.0 => TextBounds {
            left: x.floor() as i32,
            top: y.floor() as i32,
            right: (x + w).ceil() as i32,
            bottom: (y + h).ceil() as i32,
        },
        Some(_) => TextBounds {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        None => TextBounds::default(),
    }
}

/// Resolve a scene text run to a queued run. No shaping here and no text
/// synthesis: the string lives in the shared registry; this maps placement,
/// color, opacity and clip. Unknown ids are NOT resolved here — the render
/// side skips them explicitly at encode time.
///
/// Color convention (verified against glyphon 0.12 source): the atlas runs
/// `ColorMode::Accurate`, so glyphon treats `default_color` bytes as sRGB
/// and linearizes in-shader. We therefore pass `Oklch::to_srgb8` — passing
/// linear bytes would double-linearize (too dark; white primaries hide it).
/// The render target is the linear working texture, matching the rect
/// pipeline's `to_gpu()` output. Alpha is premultiplied by the item
/// opacity BEFORE 8-bit quantization.
pub fn resolve_scene_text(
    run: &crate::scene::TextRun,
    at_logical: [f32; 2],
    color: Oklch,
    opacity: f32,
    clip_logical: Option<Rect>,
    scale: f32,
) -> ShapedRun {
    let mut c = color;
    c.alpha = (c.alpha * opacity).clamp(0.0, 1.0);
    let [r, g, b, a] = c.to_srgb8();
    ShapedRun {
        id: run.id,
        revision: run.revision,
        at_physical: [at_logical[0] * scale, at_logical[1] * scale],
        color_glyphon: glyphon::Color::rgba(r, g, b, a),
        clip_physical: clip_logical.map(|c| [c.x * scale, c.y * scale, c.w * scale, c.h * scale]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aqua_text::{TextLayout, TextStyle};

    fn laid_out(s: &str) -> TextLayout {
        let mut t = TextLayout::new();
        t.set_text(s, &TextStyle::default());
        // NOTE: no layout() call here (needs real fonts): the registry
        // decision under test is revision coherence, not shaping.
        t
    }

    fn queued(id: u64, revision: u64) -> ShapedRun {
        ShapedRun {
            id,
            revision,
            at_physical: [0.0, 0.0],
            color_glyphon: glyphon::Color::rgba(255, 255, 255, 255),
            clip_physical: None,
        }
    }

    #[test]
    fn stale_revision_never_yields_current_layout() {
        let mut reg = RunRegistry::new();
        let (id, rev0) = reg.insert(laid_out("AAA"));
        let rev1 = reg.update(id, laid_out("BBBBBB")).unwrap();
        assert_ne!(rev0, rev1);

        // Old scene revision: strict skip. The Err carries only the live
        // revision number — no layout handle — so encode has nothing to
        // paint and "BBBBBB" can never appear under rev0.
        match resolve_for_encode(&reg, &queued(id.0, rev0)) {
            Err(SkipReason::StaleRevision { current }) => assert_eq!(current, rev1),
            _ => panic!("expected StaleRevision skip, got Ok"),
        }

        // Live revision resolves to the current text only.
        let handle = resolve_for_encode(&reg, &queued(id.0, rev1)).expect("live resolves");
        assert_eq!(handle.revision, rev1);

        // Unknown id skips, never resolves.
        assert!(matches!(
            resolve_for_encode(&reg, &queued(9999, 0)),
            Err(SkipReason::Unknown)
        ));
    }
}
