use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, LazyLock};
use std::time::Instant as WallInstant;

use aqua_core::geometry::{snap_device_pixel, snap_device_stroke, CornerRadii, Rect};
use aqua_core::image::ImagePlacement;
use aqua_core::scene::{DisplayItem, GpuiQuadPulse, ShadowLayer, TextRasterMode};
use aqua_core::{GpuiBackground, GpuiHsla, Oklch, Point, Size, Transform2D};
use aqua_graph::tone::luminance;
use aqua_graph::{
    plan_paint_with, BackdropBlur, Extent, FramePlan, Glyph, GlyphRun, Gradient, GradientQuad,
    GradientStop, Image, Linear2, PaintCommand, PlanError, PlanLimits, Quad, Shadow, SlugInstance,
    SlugRun,
};
use aqua_media::atlas::{AtlasConfig, ImageKey, Placement, Slot, StreamBudget};
use aqua_media::reduce::{reduce, reduction_level};
use aqua_media::residency::{GlyphSlot, Residency, ResidencyConfig, VectorMode};
use aqua_media::upload::texture_handle;
use aqua_protocol::{Instant, UploadBatch};
use aqua_scene::Scene;
use aqua_text::raster::{quantize_size, quantize_x, raster_key};
use aqua_text::residency::GlyphResolver;
use aqua_text::runs::RunId;
use aqua_text::slug::{SlugCache, SlugStore, StoreLookup, STORE_HANDLE};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_transport::{LowerContext, Lowered as Commands, TrimRequest};

use super::affine::Affine;
use super::color;
use super::host::RenderStats;
use super::link::ExternalUpdate;
use super::mesh;
use super::shape::{self, Border, Fidelity, Paint, Radii, ShadowSpec, Sides};
use super::text::{ShimmerClocks, TextScope, TextStack};
use super::transition::{Phase, Settings, Transition};
use super::vector::{gradient_axis, rotation_steps, VectorCache};

const MESH_TAG: u64 = 0xC000_0000_0000_0000;
const ROUNDED_CLIP_AREA_CAP: f32 = 0.75;
const SLUG_TEXT_PX: f32 = 160.0;
const OPAQUE_FILL: f32 = 0.98;
const FILL_MEMORY: usize = 256;
const FILL_SEARCH: usize = 64;

static PULSE_EPOCH: LazyLock<WallInstant> = LazyLock::new(WallInstant::now);

#[must_use]
pub fn gpui_pulse_clock_ms() -> u32 {
    PULSE_EPOCH.elapsed().as_millis() as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unsupported {
    VectorFailed,
    PathFailed,
    MeshTooLarge,
    MeshFailed,
    GlyphBudget,
    UnbalancedScope,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LowerReport {
    pub items: u32,
    pub commands: u32,
    pub glyphs: u32,
    pub curves: u32,
    pub approximated: u32,
    pub pending: u32,
    pub pulsing: bool,
    pub shimmering: bool,
    pub skipped: BTreeMap<Unsupported, u32>,
}

impl LowerReport {
    #[must_use]
    pub fn unsupported(&self) -> u32 {
        self.skipped.values().sum()
    }
}

#[derive(Debug)]
pub struct Lowered {
    pub uploads: UploadBatch,
    pub plan: FramePlan,
    pub report: LowerReport,
}

#[derive(Clone, Debug, Default)]
pub struct RecoverySignal {
    listened: Arc<AtomicBool>,
    lost: Arc<AtomicBool>,
}

impl RecoverySignal {
    #[must_use]
    pub fn listener(&self) -> Self {
        self.listened.store(true, Ordering::Release);
        self.clone()
    }

    pub fn textures_lost(&self) {
        self.lost.store(true, Ordering::Release);
    }

    fn consume(&self) -> bool {
        let lost = self.lost.swap(false, Ordering::AcqRel);
        lost || !self.listened.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
struct RetainedImage {
    width: u32,
    height: u32,
    premultiplied: Arc<[u8]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Clip,
    Opacity,
    Blur,
}

struct Walk {
    base: Affine,
    transforms: Vec<Affine>,
    open: Vec<Scope>,
    out: Vec<PaintCommand>,
    report: LowerReport,
    surface: Rect,
    now_secs: f64,
    pulses: Vec<GpuiQuadPulse>,
    lifted: bool,
    fills: Vec<(Rect, f32)>,
    clear_luminance: Option<f32>,
}

impl Walk {
    fn remember_fill(&mut self, bounds: Rect, paint: Paint) {
        if let Paint::Solid(color) = paint {
            if color.alpha >= OPAQUE_FILL && !self.lifted {
                let [r, g, b, _] = color::srgb(color);
                if self.fills.len() >= FILL_MEMORY * 2 {
                    self.fills.drain(..FILL_MEMORY);
                }
                self.fills.push((bounds, luminance([r, g, b])));
            }
        }
    }

    fn backdrop_at(&self, x: f32, y: f32) -> Option<f32> {
        let point = Point::new(x, y);
        self.fills
            .iter()
            .rev()
            .take(FILL_SEARCH)
            .find(|(bounds, _)| bounds.contains(point))
            .map(|(_, luminance)| *luminance)
            .or(self.clear_luminance)
    }

    fn flat_radii(&mut self, radii: Radii, bounds: Rect) -> Radii {
        match self.lifted && !radii.is_uniform() {
            true => {
                self.report.approximated += 1;
                Radii::uniform(radii.fitted(bounds).max())
            }
            false => radii,
        }
    }

    fn flat_border(&mut self, border: Border) -> Border {
        match self.lifted && (!border.widths.is_uniform() || border.dashed) {
            true => {
                self.report.approximated += 1;
                Border {
                    widths: Sides::uniform(border.widths.max()),
                    dashed: false,
                    ..border
                }
            }
            false => border,
        }
    }

    fn at(&self) -> Affine {
        self.transforms.last().copied().unwrap_or(self.base)
    }

    fn close(&mut self, kind: Scope) -> bool {
        match self.open.last() {
            Some(top) if *top == kind => {
                self.open.pop();
                true
            }
            _ => false,
        }
    }

    fn skip(&mut self, reason: Unsupported) {
        *self.report.skipped.entry(reason).or_insert(0) += 1;
    }

    fn note(&mut self, fidelity: Fidelity) {
        if fidelity == Fidelity::Approximated {
            self.report.approximated += 1;
        }
    }

    fn pulse_factor(&mut self) -> f32 {
        match self.pulses.last().copied() {
            Some(pulse) if pulse.period_ms != 0 => {
                self.report.pulsing = true;
                let elapsed = gpui_pulse_clock_ms().wrapping_sub(pulse.start_ms) % pulse.period_ms;
                let progress = elapsed as f32 / pulse.period_ms as f32;
                let swing = (progress * std::f32::consts::TAU).cos() * 0.5 + 0.5;
                let minimum = pulse.min_opacity.clamp(0.0, 1.0);
                minimum + (1.0 - minimum) * swing
            }
            _ => 1.0,
        }
    }
}

pub struct Lowering {
    fonts: SharedFonts,
    runs: SharedRuns,
    residency: Residency,
    resolver: GlyphResolver,
    slug_cache: SlugCache,
    slug_store: SlugStore,
    retained: HashMap<u64, RetainedImage>,
    meshes: HashMap<u64, ImageKey>,
    reduced: HashMap<u64, Vec<ImageKey>>,
    vectors: VectorCache,
    text: TextStack,
    shimmer: ShimmerClocks,
    externals: Option<Receiver<ExternalUpdate>>,
    recovery: RecoverySignal,
    stats: Option<Arc<RenderStats>>,
    report_pages: u32,
    clear: Oklch,
    last_now: Instant,
    last_report: LowerReport,
    last_commands: Vec<PaintCommand>,
    begun: Option<Settings>,
    transition: Option<Transition>,
    outline_text: bool,
}

fn residency_config() -> ResidencyConfig {
    ResidencyConfig {
        masks: AtlasConfig::glyphs(),
        colors: AtlasConfig::images(),
        stream: StreamBudget {
            bytes_per_frame: u64::MAX,
        },
        ..ResidencyConfig::default()
    }
}

fn premultiply(rgba: &[u8]) -> Arc<[u8]> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let alpha = u16::from(pixel[3]);
            let scale = |channel: u8| ((u16::from(channel) * alpha + 127) / 255) as u8;
            [scale(pixel[0]), scale(pixel[1]), scale(pixel[2]), pixel[3]]
        })
        .collect::<Vec<u8>>()
        .into()
}

fn texel_rect(placement: &Placement, source: Rect) -> Rect {
    Rect {
        x: placement.x as f32 + source.x * placement.width as f32,
        y: placement.y as f32 + source.y * placement.height as f32,
        w: source.w * placement.width as f32,
        h: source.h * placement.height as f32,
    }
}

fn physical_extent(size: Size, scale: f32) -> Extent {
    Extent {
        width: (size.w * scale).round().max(1.0) as u32,
        height: (size.h * scale).round().max(1.0) as u32,
    }
}

fn snapped(rect: Rect) -> Rect {
    let left = snap_device_pixel(rect.x, 1.0);
    let top = snap_device_pixel(rect.y, 1.0);
    let right = snap_device_pixel(rect.right(), 1.0).max(left);
    let bottom = snap_device_pixel(rect.bottom(), 1.0).max(top);
    Rect::from_edges(left, top, right, bottom)
}

fn outward(rect: Rect) -> Rect {
    Rect::from_edges(
        rect.x.floor(),
        rect.y.floor(),
        rect.right().ceil(),
        rect.bottom().ceil(),
    )
}

fn with_factor(color: Oklch, factor: f32) -> Oklch {
    match factor >= 1.0 {
        true => color,
        false => color.with_alpha(color.alpha * factor),
    }
}

fn scale_paint(paint: Paint, factor: f32) -> Paint {
    match paint {
        Paint::Solid(color) => Paint::Solid(with_factor(color, factor)),
        Paint::Gradient(gradient) => Paint::Gradient(Gradient {
            stops: gradient.stops.map(|stop| GradientStop {
                offset: stop.offset,
                color: with_factor(stop.color, factor),
            }),
            ..gradient
        }),
    }
}

fn hsla_oklch(color: GpuiHsla) -> Oklch {
    color::oklch(color::hsla(color))
}

fn background_paint(background: GpuiBackground, bounds: Rect) -> Paint {
    match background {
        GpuiBackground::Solid(solid) => Paint::Solid(hsla_oklch(solid)),
        GpuiBackground::LinearGradient { angle, stops, .. } => {
            let (start, end) = gradient_axis(bounds, angle);
            Paint::Gradient(Gradient {
                start: [start.x, start.y],
                end: [end.x, end.y],
                stops: stops.map(|stop| GradientStop {
                    offset: stop.percentage,
                    color: hsla_oklch(stop.color),
                }),
            })
        }
    }
}

fn border_sides(widths: aqua_core::Insets, scale: f32) -> Sides {
    Sides {
        top: snap_device_stroke(widths.top, scale),
        right: snap_device_stroke(widths.right, scale),
        bottom: snap_device_stroke(widths.bottom, scale),
        left: snap_device_stroke(widths.left, scale),
    }
}

fn shadow_spec(layer: &ShadowLayer, color: Oklch, scale: f32) -> ShadowSpec {
    ShadowSpec {
        dx: layer.dx * scale,
        dy: layer.dy * scale,
        sigma: layer.blur.max(0.0) * scale * 0.5,
        spread: layer.spread * scale,
        color,
        inset: layer.inset,
    }
}

fn full_source() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    }
}

fn push_image(
    out: &mut Vec<PaintCommand>,
    placement: &Placement,
    bounds: Rect,
    source: Rect,
    opacity: f32,
    radius: f32,
) {
    let wrap = opacity < 1.0;
    if wrap {
        out.push(PaintCommand::PushOpacity(opacity.clamp(0.0, 1.0)));
    }
    out.push(PaintCommand::Image(Image {
        source: texture_handle(placement.page),
        bounds,
        texels: texel_rect(placement, source),
        radius,
    }));
    if wrap {
        out.push(PaintCommand::PopOpacity);
    }
}

fn image_key(layer: u64, image: &RetainedImage) -> ImageKey {
    ImageKey {
        asset: layer,
        frame: 0,
        width: image.width,
        height: image.height,
    }
}

impl Lowering {
    pub fn new(fonts: SharedFonts, runs: SharedRuns) -> Self {
        Self {
            fonts,
            runs,
            residency: Residency::new(residency_config()),
            resolver: GlyphResolver::new(),
            slug_cache: SlugCache::new(),
            slug_store: SlugStore::new(),
            retained: HashMap::new(),
            meshes: HashMap::new(),
            reduced: HashMap::new(),
            vectors: VectorCache::default(),
            text: TextStack::default(),
            shimmer: ShimmerClocks::default(),
            externals: None,
            recovery: RecoverySignal::default(),
            stats: None,
            report_pages: 0,
            clear: Oklch::TRANSPARENT,
            last_now: Instant::ZERO,
            last_report: LowerReport::default(),
            last_commands: Vec::new(),
            begun: None,
            transition: None,
            outline_text: false,
        }
    }

    #[must_use]
    pub fn with_clear(mut self, clear: Oklch) -> Self {
        self.clear = clear;
        self
    }

    #[must_use]
    pub fn with_externals(mut self, externals: Receiver<ExternalUpdate>) -> Self {
        self.externals = Some(externals);
        self
    }

    #[must_use]
    pub fn with_stats(mut self, stats: Arc<RenderStats>) -> Self {
        self.stats = Some(stats);
        self
    }

    #[must_use]
    pub fn with_recovery(mut self, signal: RecoverySignal) -> Self {
        self.recovery = signal;
        self
    }

    #[must_use]
    pub fn clear_color(&self) -> Oklch {
        self.clear
    }

    #[must_use]
    pub fn last_report(&self) -> &LowerReport {
        &self.last_report
    }

    pub fn register_image(&mut self, layer: u64, width: u32, height: u32, rgba: &[u8]) {
        if width == 0 || height == 0 || rgba.len() != (width as usize) * (height as usize) * 4 {
            return;
        }
        self.release_image(layer);
        self.retained.insert(
            layer,
            RetainedImage {
                width,
                height,
                premultiplied: premultiply(rgba),
            },
        );
    }

    pub fn release_image(&mut self, layer: u64) {
        if let Some(image) = self.retained.remove(&layer) {
            self.residency.remove_image(&image_key(layer, &image));
        }
        self.reduced.remove(&layer).into_iter().flatten().for_each(|key| {
            self.residency.remove_image(&key);
        });
    }

    pub fn reset(&mut self) {
        self.residency = Residency::new(residency_config());
        self.resolver = GlyphResolver::new();
        self.slug_cache.clear();
        self.slug_store.reset();
        self.meshes.clear();
        self.reduced.clear();
        self.vectors.clear();
    }

    pub fn trim(&mut self, idle_frames: u64) -> usize {
        self.residency.trim_idle(idle_frames)
    }

    pub fn residency(&self) -> &Residency {
        &self.residency
    }

    pub fn at(&mut self, now: Instant) -> &mut Self {
        self.last_now = now;
        self
    }

    pub fn commands(
        &mut self,
        scene: &Scene,
        size: Size,
        scale: f32,
        now: Instant,
    ) -> (Vec<PaintCommand>, UploadBatch, LowerReport) {
        let started = WallInstant::now();
        self.last_now = now;
        let mut previous = std::mem::take(&mut self.last_commands);
        self.residency.begin_frame();
        self.shimmer.begin_frame();
        self.text.clear();
        let extent = physical_extent(size, scale);
        let mut walk = Walk {
            base: Affine::device(scale),
            transforms: Vec::new(),
            open: Vec::new(),
            out: Vec::with_capacity(scene.len() * 2),
            report: LowerReport::default(),
            surface: Rect {
                x: 0.0,
                y: 0.0,
                w: extent.width as f32,
                h: extent.height as f32,
            },
            now_secs: now.as_secs_f64(),
            pulses: Vec::new(),
            lifted: false,
            fills: Vec::new(),
            clear_luminance: Some(self.clear)
                .filter(|clear| clear.alpha >= OPAQUE_FILL)
                .map(|clear| {
                    let [r, g, b, _] = color::srgb(clear);
                    luminance([r, g, b])
                }),
        };
        for (_, item) in scene.ordered() {
            walk.report.items += 1;
            self.emit(item, &mut walk);
        }
        while let Some(kind) = walk.open.pop() {
            walk.out.push(match kind {
                Scope::Clip => PaintCommand::PopClip,
                Scope::Opacity => PaintCommand::PopOpacity,
                Scope::Blur => PaintCommand::PopContentBlur,
            });
        }
        walk.report.shimmering = self.shimmer.active();
        self.shimmer.end_frame();
        self.adopt_previous(&previous);
        previous.clone_from(&walk.out);
        self.last_commands = previous;
        walk.report.shimmering |= self.overlay_transition(&mut walk.out, walk.surface, now);
        walk.report.commands = walk.out.len() as u32;
        let updates = self.residency.finish_frame();
        let mut uploads = aqua_media::upload::upload_batch(updates);
        uploads.textures.extend(self.slug_store.uploads().textures);
        if let Some(stats) = &self.stats {
            record_lowering(stats, &walk.report, started.elapsed());
            let resident = self.residency.stats();
            let report_signature = resident.colors.pages
                + (self.slug_store.resident_bytes() / 16384) as u32 * 64;
            if self.report_pages != report_signature && crate::mem_probe::memory_report_enabled() {
                self.report_pages = report_signature;
                eprintln!("aqua-atlas-pages: {:?}", self.residency.page_layouts());
                eprintln!("aqua-atlas-keys: {:?}", self.residency.page_keys());
                eprintln!(
                    "aqua-lowering-heap: slug_cache={} slug_store_cpu={} slug_store_gpu={} retained_vectors={} retained_images={} streaming={}",
                    self.slug_cache.heap_bytes(),
                    self.slug_store.texel_count() * 16,
                    self.slug_store.resident_bytes(),
                    resident.retained_source_bytes,
                    self.retained.values().map(|image| image.premultiplied.len()).sum::<usize>(),
                    resident.streaming_bytes,
                );
            }
            stats
                .atlas_mask_bytes
                .store(resident.masks.resident_bytes, Ordering::Relaxed);
            stats
                .atlas_color_bytes
                .store(resident.colors.resident_bytes, Ordering::Relaxed);
            stats.atlas_pages.store(
                u64::from(resident.masks.pages + resident.colors.pages),
                Ordering::Relaxed,
            );
        }
        self.last_report = walk.report.clone();
        (walk.out, uploads, walk.report)
    }

    pub fn lower(
        &mut self,
        scene: &Scene,
        size: Size,
        scale: f32,
        limits: PlanLimits,
    ) -> Result<Lowered, PlanError> {
        let now = self.last_now;
        let (commands, uploads, report) = self.commands(scene, size, scale, now);
        let plan = plan_paint_with(physical_extent(size, scale), self.clear, &commands, limits)?;
        Ok(Lowered {
            uploads,
            plan,
            report,
        })
    }

    fn emit(&mut self, item: &DisplayItem, walk: &mut Walk) {
        let current = walk.at();
        let stays = matches!(
            item,
            DisplayItem::PushTransform(_) | DisplayItem::PopTransform | DisplayItem::Text { .. }
        );
        match current.is_exact() || stays {
            true => self.emit_item(item, walk),
            false => {
                let lift = Transform2D::scale(1.0 / walk.base.scale(), 1.0 / walk.base.scale())
                    .then(current.0);
                let saved = std::mem::take(&mut walk.transforms);
                walk.lifted = true;
                let start = walk.out.len();
                self.emit_item(item, walk);
                walk.lifted = false;
                walk.transforms = saved;
                let approximated = lift_commands(&mut walk.out[start..], lift);
                walk.report.approximated += approximated;
            }
        }
    }

    fn emit_item(&mut self, item: &DisplayItem, walk: &mut Walk) {
        let at = walk.at();
        let s = at.scale();
        match item {
            DisplayItem::Rect {
                bounds,
                radius,
                fill,
                border_width,
                border_color,
                shadow,
            } => {
                let bounds = at.rect(*bounds);
                let radii = Radii::uniform(radius * s);
                if let Some((ox, oy, blur, color)) = shadow {
                    let spec = ShadowSpec {
                        dx: ox * s,
                        dy: oy * s,
                        sigma: blur * s * 0.5,
                        spread: 0.0,
                        color: *color,
                        inset: false,
                    };
                    let fidelity = shape::emit_shadow(&mut walk.out, bounds, radii, spec);
                    walk.note(fidelity);
                }
                let border = Border {
                    widths: Sides::uniform(border_width * s),
                    color: *border_color,
                    dashed: false,
                };
                walk.remember_fill(bounds, Paint::Solid(*fill));
                shape::emit_box(
                    &mut walk.out,
                    bounds,
                    radii,
                    Some(Paint::Solid(*fill)),
                    Some(border),
                );
            }
            DisplayItem::Quad {
                bounds,
                corners,
                fill,
                border_widths,
                border_color,
                dashed,
            } => {
                let factor = walk.pulse_factor();
                let bounds = snapped(at.rect(*bounds));
                let border = walk.flat_border(Border {
                    widths: border_sides(*border_widths, s),
                    color: with_factor(*border_color, factor),
                    dashed: *dashed,
                });
                let radii = walk.flat_radii(Radii::scaled(*corners, s), bounds);
                let paint = scale_paint(Paint::Solid(*fill), factor);
                walk.remember_fill(bounds, paint);
                shape::emit_box(&mut walk.out, bounds, radii, Some(paint), Some(border));
            }
            DisplayItem::GpuiBackground {
                bounds,
                corners,
                background,
            } => {
                let factor = walk.pulse_factor();
                let bounds = snapped(at.rect(*bounds));
                let paint = scale_paint(background_paint(*background, bounds), factor);
                let radii = walk.flat_radii(Radii::scaled(*corners, s), bounds);
                walk.remember_fill(bounds, paint);
                shape::emit_box(&mut walk.out, bounds, radii, Some(paint), None);
            }
            DisplayItem::GpuiBorder {
                bounds,
                corners,
                widths,
                color,
                dashed,
            } => {
                let factor = walk.pulse_factor();
                let border = walk.flat_border(Border {
                    widths: border_sides(*widths, s),
                    color: with_factor(hsla_oklch(*color), factor),
                    dashed: *dashed,
                });
                let bounds = snapped(at.rect(*bounds));
                let radii = walk.flat_radii(Radii::scaled(*corners, s), bounds);
                shape::emit_box(&mut walk.out, bounds, radii, None, Some(border));
            }
            DisplayItem::Shadow {
                bounds,
                corners,
                layer,
            } => {
                let fidelity = shape::emit_shadow(
                    &mut walk.out,
                    outward(at.rect(*bounds)),
                    Radii::scaled(*corners, s),
                    shadow_spec(layer, layer.color, s),
                );
                walk.note(fidelity);
            }
            DisplayItem::GpuiShadow {
                bounds,
                corners,
                layer,
            } => {
                let geometry = ShadowLayer {
                    dx: layer.dx,
                    dy: layer.dy,
                    blur: layer.blur,
                    spread: layer.spread,
                    inset: layer.inset,
                    color: Oklch::TRANSPARENT,
                };
                let fidelity = shape::emit_shadow(
                    &mut walk.out,
                    outward(at.rect(*bounds)),
                    Radii::scaled(*corners, s),
                    shadow_spec(&geometry, hsla_oklch(layer.color), s),
                );
                walk.note(fidelity);
            }
            DisplayItem::PushClip { bounds, radius } => {
                let rect = at.rect(*bounds);
                self.push_clip(walk, rect, Radii::uniform(radius * s));
            }
            DisplayItem::PushRoundedClip { bounds, corners } => {
                let rect = at.rect(*bounds);
                self.push_clip(walk, rect, Radii::scaled(*corners, s));
            }
            DisplayItem::PushGpuiContentMask { bounds } => {
                walk.open.push(Scope::Clip);
                walk.out
                    .push(PaintCommand::PushClip(outward(at.rect(*bounds))));
            }
            DisplayItem::PopClip => match walk.close(Scope::Clip) {
                true => walk.out.push(PaintCommand::PopClip),
                false => walk.skip(Unsupported::UnbalancedScope),
            },
            DisplayItem::PushOpacity(opacity) => {
                walk.open.push(Scope::Opacity);
                walk.out
                    .push(PaintCommand::PushOpacity(opacity.clamp(0.0, 1.0)));
            }
            DisplayItem::PopOpacity => match walk.close(Scope::Opacity) {
                true => walk.out.push(PaintCommand::PopOpacity),
                false => walk.skip(Unsupported::UnbalancedScope),
            },
            DisplayItem::PushContentBlur { bounds, radius } => {
                walk.open.push(Scope::Blur);
                walk.out.push(PaintCommand::PushContentBlur {
                    bounds: at.rect(*bounds),
                    radius: radius * s,
                });
            }
            DisplayItem::PopContentBlur => match walk.close(Scope::Blur) {
                true => walk.out.push(PaintCommand::PopContentBlur),
                false => walk.skip(Unsupported::UnbalancedScope),
            },
            DisplayItem::BackdropBlur { bounds, radius } => {
                walk.out.push(PaintCommand::BackdropBlur(BackdropBlur {
                    bounds: at.rect(*bounds),
                    radius: radius * s,
                    corner_radius: 0.0,
                }));
            }
            DisplayItem::BackdropBlurRounded {
                bounds,
                radius,
                corners,
            } => {
                let radii = Radii::scaled(*corners, s);
                walk.note(match radii.is_uniform() {
                    true => Fidelity::Exact,
                    false => Fidelity::Approximated,
                });
                walk.out.push(PaintCommand::BackdropBlur(BackdropBlur {
                    bounds: at.rect(*bounds),
                    radius: radius * s,
                    corner_radius: radii.max(),
                }));
            }
            DisplayItem::PushTransform(local) => {
                let next = at.nested(*local);
                let usable = next.finite() && next.scale() > 0.0;
                walk.note(match usable {
                    true => Fidelity::Exact,
                    false => Fidelity::Approximated,
                });
                walk.transforms.push(match usable {
                    true => next,
                    false => at,
                });
            }
            DisplayItem::PopTransform => {
                if walk.transforms.pop().is_none() {
                    walk.skip(Unsupported::UnbalancedScope);
                }
            }
            DisplayItem::Text {
                at: origin,
                run,
                color,
                opacity,
            } => self.emit_text(*origin, run.id, *color, *opacity, walk),
            DisplayItem::Image {
                bounds,
                layer,
                fit,
                position,
                source,
                opacity,
                rounded_clip,
            } => self.emit_image(
                *layer,
                at.rect(*bounds),
                Some((*fit, *position, *source)),
                *opacity,
                *rounded_clip * s,
                walk,
            ),
            DisplayItem::External {
                bounds,
                layer,
                opacity,
                rounded_clip,
            } => self.emit_image(
                *layer,
                at.rect(*bounds),
                None,
                *opacity,
                *rounded_clip * s,
                walk,
            ),
            DisplayItem::Vector {
                bounds,
                source,
                opacity,
            } => self.emit_vector(source.clone(), at.rect(*bounds), *opacity, walk),
            DisplayItem::GpuiSvg {
                bounds,
                source,
                tint,
                opacity,
            } => {
                let vector = self.vectors.svg(source, tint.map(hsla_oklch), 0);
                self.emit_vector(vector, at.rect(*bounds), *opacity, walk);
            }
            DisplayItem::GpuiTransformedSvg {
                bounds,
                source,
                tint,
                opacity,
                transform,
            } => {
                let center = bounds.center();
                let width = bounds.w * transform.scale.x;
                let height = bounds.h * transform.scale.y;
                let placed = Rect {
                    x: center.x + transform.translation.x - width * 0.5,
                    y: center.y + transform.translation.y - height * 0.5,
                    w: width,
                    h: height,
                };
                let vector = self.vectors.svg(
                    source,
                    Some(hsla_oklch(*tint)),
                    rotation_steps(transform.rotation_radians),
                );
                self.emit_vector(vector, at.rect(placed), *opacity, walk);
            }
            DisplayItem::GpuiPath { path, .. } => self.emit_path(path, at, walk),
            DisplayItem::GpuiMesh {
                bounds,
                corners,
                mesh,
                style,
            } => self.emit_mesh(snapped(at.rect(*bounds)), *corners, mesh, style, s, walk),
            DisplayItem::PushGpuiQuadPulse(pulse) => walk.pulses.push(*pulse),
            DisplayItem::PopGpuiQuadPulse => {
                if walk.pulses.pop().is_none() {
                    walk.skip(Unsupported::UnbalancedScope);
                }
            }
            DisplayItem::PushTextColorMap(map) => self.text.push_legacy(*map),
            DisplayItem::PopTextColorMap => self.pop_text(TextScope::LegacyMap, walk),
            DisplayItem::PushGpuiTextColorMap(map) => self.text.push_map(*map, None),
            DisplayItem::PushGpuiTextShimmer(shimmer) => {
                self.text.push_map(shimmer.map, Some(*shimmer))
            }
            DisplayItem::PopGpuiTextColorMap => self.pop_text(TextScope::Map, walk),
            DisplayItem::PushGpuiTextColor(color) => self.text.push_color(*color),
            DisplayItem::PopGpuiTextColor => self.pop_text(TextScope::Color, walk),
            DisplayItem::PushGpuiTextOpacity(ranges) => self.text.push_opacity(ranges.clone()),
            DisplayItem::PopGpuiTextOpacity => self.pop_text(TextScope::Opacity, walk),
            DisplayItem::PushGpuiSourceTextColor(color) => self.text.push_source(*color),
            DisplayItem::PopGpuiSourceTextColor => self.pop_text(TextScope::SourceColor, walk),
        }
    }

    fn pop_text(&mut self, scope: TextScope, walk: &mut Walk) {
        if !self.text.pop(scope) {
            walk.skip(Unsupported::UnbalancedScope);
        }
    }

    fn push_clip(&mut self, walk: &mut Walk, rect: Rect, radii: Radii) {
        let fitted = radii.fitted(rect);
        let radius = fitted.max();
        let area = rect.w * rect.h;
        let cheap = area <= walk.surface.w * walk.surface.h * ROUNDED_CLIP_AREA_CAP;
        walk.open.push(Scope::Clip);
        match (radius >= 0.75, cheap) {
            (true, true) => {
                walk.note(match fitted.is_uniform() {
                    true => Fidelity::Exact,
                    false => Fidelity::Approximated,
                });
                walk.out
                    .push(PaintCommand::PushRoundedClip { rect, radius });
            }
            (true, false) => {
                walk.note(Fidelity::Approximated);
                walk.out.push(PaintCommand::PushClip(rect));
            }
            (false, _) => walk.out.push(PaintCommand::PushClip(rect)),
        }
    }

    fn emit_text(
        &mut self,
        origin: [f32; 2],
        run_id: u64,
        scene_color: Oklch,
        opacity: f32,
        walk: &mut Walk,
    ) {
        let shared = {
            let guard = self.runs.lock();
            guard
                .resolve(RunId(run_id))
                .and_then(|handle| handle.layout.laid_out_shared())
        };
        let Some(laid) = shared else {
            walk.report.pending += 1;
            return;
        };
        let at = walk.at();
        let s = at.scale();
        let (base_x, base_y) = at.point(origin[0], origin[1]);
        let mapped = self.text.has_map();
        let plain = !mapped && !self.text.has_base();
        let phase = match self.text.shimmer() {
            Some(shimmer) => self.shimmer.phase(&shimmer, walk.now_secs),
            None => 0.0,
        };
        let map_bounds = self
            .text
            .top_map()
            .map_or(walk.surface, |map| at.rect(map.bounds));
        let mut batches: Vec<(aqua_media::atlas::PageId, Vec<Glyph>)> = Vec::new();
        let mut color_images: Vec<PaintCommand> = Vec::new();
        let mut last_color: Option<([f32; 4], Oklch)> = None;
        let moving = !walk.transforms.is_empty()
            && (!at.is_exact() || (s / walk.base.scale() - 1.0).abs() > 1.0e-3);
        let first_baseline = laid.lines.first().map_or(0.0, |line| line.baseline_y);
        let backdrop = walk.backdrop_at(base_x + 1.0, base_y + first_baseline * s * 0.5);
        let mut curves: Vec<SlugInstance> = Vec::new();
        for line in &laid.lines {
            for glyph in &line.glyphs {
                if moving || self.outline_text || glyph.font_size_px * s >= SLUG_TEXT_PX {
                    let key = raster_key(glyph, 0);
                    let looked = {
                        let mut fonts = self.fonts.lock();
                        self.slug_store
                            .lookup(&mut self.slug_cache, &mut fonts, &key)
                    };
                    match looked {
                        StoreLookup::Stored(stored) => {
                            let factor = opacity * self.text.range_opacity(glyph);
                            let center = base_x + (glyph.x + glyph.x_offset + glyph.w * 0.5) * s;
                            let tinted = self.tint(
                                glyph,
                                scene_color,
                                factor,
                                (plain, phase, map_bounds, s, center),
                                &mut last_color,
                            );
                            let pen = at.point(
                                origin[0] + glyph.x + glyph.x_offset,
                                origin[1] + line.baseline_y + glyph.y - glyph.y_offset,
                            );
                            let size = glyph.font_size_px;
                            curves.push(SlugInstance {
                                origin: [pen.0, pen.1],
                                linear: Linear2::scale(size, -size).then(Linear2::of(at.0)),
                                glyph: stored.index,
                                em_bounds: stored.em_bounds,
                                color: tinted,
                                clip: None,
                                opacity: 1.0,
                                backdrop,
                            });
                            walk.report.glyphs += 1;
                            walk.report.curves += 1;
                            continue;
                        }
                        StoreLookup::Blank => continue,
                        StoreLookup::Unsupported => {}
                    }
                }
                let x = base_x + (glyph.x + glyph.x_offset) * s;
                let (pixel_x, subpixel) = quantize_x(x);
                let pen_y =
                    (base_y + (line.baseline_y + glyph.y - glyph.y_offset) * s).round() as i32;
                let mut key = raster_key(glyph, subpixel);
                key.size_bits = quantize_size(glyph.font_size_px * s).to_bits();
                let resolved = {
                    let mut fonts = self.fonts.lock();
                    self.resolver.resolve(&mut self.residency, &mut fonts, &key)
                };
                let slot = match resolved {
                    Ok(slot) => slot,
                    Err(_) => {
                        walk.skip(Unsupported::GlyphBudget);
                        continue;
                    }
                };
                let (GlyphSlot::Mask(Slot::Resident(placement))
                | GlyphSlot::Color(Slot::Resident(placement))) = slot
                else {
                    continue;
                };
                walk.report.glyphs += 1;
                let bounds = Rect {
                    x: (pixel_x + placement.bearing[0]) as f32,
                    y: (pen_y - placement.bearing[1]) as f32,
                    w: placement.width as f32,
                    h: placement.height as f32,
                };
                match slot {
                    GlyphSlot::Mask(_) => {
                        let factor = opacity * self.text.range_opacity(glyph);
                        let center = x + glyph.w * s * 0.5;
                        let tinted = self.tint(
                            glyph,
                            scene_color,
                            factor,
                            (plain, phase, map_bounds, s, center),
                            &mut last_color,
                        );
                        let entry = Glyph {
                            bounds,
                            texels: texel_rect(&placement, full_source()),
                            color: tinted,
                            backdrop,
                        };
                        match batches.last_mut() {
                            Some((page, glyphs)) if *page == placement.page => glyphs.push(entry),
                            _ => batches.push((placement.page, vec![entry])),
                        }
                    }
                    GlyphSlot::Color(_) => {
                        push_image(
                            &mut color_images,
                            &placement,
                            bounds,
                            full_source(),
                            opacity,
                            0.0,
                        );
                    }
                }
            }
        }
        for (page, glyphs) in batches {
            walk.out.push(PaintCommand::Glyphs(GlyphRun {
                atlas: texture_handle(page),
                glyphs,
            }));
        }
        if !curves.is_empty() {
            walk.out.push(PaintCommand::Slug(SlugRun {
                store: STORE_HANDLE,
                glyphs: curves,
            }));
        }
        walk.out.extend(color_images);
    }

    fn tint(
        &self,
        glyph: &aqua_text::layout::LaidGlyph,
        scene_color: Oklch,
        factor: f32,
        (plain, phase, map_bounds, scale, center): (bool, f32, Rect, f32, f32),
        last_color: &mut Option<([f32; 4], Oklch)>,
    ) -> Oklch {
        match plain && glyph_is_plain(glyph) {
            true => with_factor(scene_color, factor),
            false => {
                let base = self.text.base(glyph, scene_color);
                let shaded = self.text.mapped(base, center, phase, map_bounds, scale);
                let finished = color::with_alpha(shaded, factor);
                match *last_color {
                    Some((previous, converted)) if previous == finished => converted,
                    _ => {
                        let converted = color::oklch(finished);
                        *last_color = Some((finished, converted));
                        converted
                    }
                }
            }
        }
    }

    fn emit_image(
        &mut self,
        layer: u64,
        bounds: Rect,
        fit: Option<(aqua_core::image::ImageFit, Point, Rect)>,
        opacity: f32,
        radius: f32,
        walk: &mut Walk,
    ) {
        let Some(image) = self.retained.get(&layer).cloned() else {
            walk.report.pending += 1;
            return;
        };
        let (destination, source, clipped) = match fit {
            None => (bounds, full_source(), false),
            Some((fit, position, source)) => {
                let natural = Size {
                    w: image.width as f32,
                    h: image.height as f32,
                };
                match ImagePlacement::resolve(bounds, natural, fit, position, source) {
                    Ok(placed) => (placed.destination, placed.source, true),
                    Err(_) => (bounds, source, false),
                }
            }
        };
        let level = reduction_level(
            image.width,
            image.height,
            destination.w / source.w.max(1e-3),
            destination.h / source.h.max(1e-3),
        );
        let key = match level {
            0 => image_key(layer, &image),
            level => ImageKey {
                asset: layer,
                frame: level,
                width: image.width >> level,
                height: image.height >> level,
            },
        };
        let slot = match self.residency.image_resident(&key) {
            Some(slot) => Ok(slot),
            None => match level {
                0 => self.residency.image(key, &image.premultiplied),
                level => {
                    let reduced = reduce(&image.premultiplied, image.width, image.height, level);
                    let levels = self.reduced.entry(layer).or_default();
                    if !levels.contains(&key) {
                        levels.push(key);
                    }
                    self.residency.image(key, &reduced.pixels)
                }
            },
        };
        let Ok(Slot::Resident(placement)) = slot else {
            walk.report.pending += 1;
            return;
        };
        let overflow = clipped
            && (destination.x < bounds.x
                || destination.y < bounds.y
                || destination.right() > bounds.right()
                || destination.bottom() > bounds.bottom());
        if overflow {
            walk.out.push(PaintCommand::PushClip(bounds));
        }
        push_image(
            &mut walk.out,
            &placement,
            destination,
            source,
            opacity,
            radius,
        );
        if overflow {
            walk.out.push(PaintCommand::PopClip);
        }
    }

    fn emit_vector(
        &mut self,
        source: aqua_core::vector::VectorSource,
        physical: Rect,
        opacity: f32,
        walk: &mut Walk,
    ) {
        let width = physical.w.round().max(1.0) as u32;
        let height = physical.h.round().max(1.0) as u32;
        match self
            .residency
            .vector(&source, width, height, VectorMode::Color)
        {
            Ok(Slot::Resident(placement)) => push_image(
                &mut walk.out,
                &placement,
                physical,
                full_source(),
                opacity,
                0.0,
            ),
            Ok(Slot::Blank { .. }) => {}
            Err(_) => walk.skip(Unsupported::VectorFailed),
        }
    }

    fn emit_path(&mut self, path: &aqua_core::vector::GpuiPath, at: Affine, walk: &mut Walk) {
        let Some(raster) = self.vectors.path(path, at.0) else {
            walk.skip(Unsupported::PathFailed);
            return;
        };
        match self.residency.vector(
            &raster.source,
            raster.width,
            raster.height,
            VectorMode::Color,
        ) {
            Ok(Slot::Resident(placement)) => push_image(
                &mut walk.out,
                &placement,
                raster.placement,
                full_source(),
                1.0,
                0.0,
            ),
            Ok(Slot::Blank { .. }) => {}
            Err(_) => walk.skip(Unsupported::PathFailed),
        }
    }

    fn emit_mesh(
        &mut self,
        bounds: Rect,
        corners: CornerRadii,
        mesh: &Arc<aqua_core::GpuiMesh>,
        style: &aqua_core::GpuiMeshStyle,
        scale: f32,
        walk: &mut Walk,
    ) {
        let (width, height) = (bounds.w.round() as u32, bounds.h.round() as u32);
        if width == 0 || height == 0 || mesh.vertices().is_empty() {
            return;
        }
        if mesh.vertices().len() / 3 > mesh::TRIANGLE_LIMIT {
            walk.skip(Unsupported::MeshTooLarge);
            return;
        }
        let key = ImageKey {
            asset: MESH_TAG | (mesh.id() & !MESH_TAG),
            frame: mesh::style_bits(style) as u32,
            width,
            height,
        };
        if let Some(previous) = self
            .meshes
            .get(&mesh.id())
            .copied()
            .filter(|old| *old != key)
        {
            self.residency.remove_image(&previous);
        }
        let slot = match self.residency.image_resident(&key) {
            Some(slot) => Ok(slot),
            None => {
                let pixels = mesh::rasterize(mesh, style, width, height);
                self.residency.image(key, &pixels)
            }
        };
        self.meshes.insert(mesh.id(), key);
        match slot {
            Ok(Slot::Resident(placement)) => {
                let radius = Radii::scaled(corners, scale).fitted(bounds).max();
                push_image(
                    &mut walk.out,
                    &placement,
                    bounds,
                    full_source(),
                    1.0,
                    radius,
                )
            }
            _ => walk.skip(Unsupported::MeshFailed),
        }
    }

    fn drain_externals(&mut self) {
        let updates: Vec<ExternalUpdate> = self
            .externals
            .as_ref()
            .map(|receiver| receiver.try_iter().collect())
            .unwrap_or_default();
        updates.into_iter().for_each(|update| match update {
            ExternalUpdate::Image {
                layer,
                width,
                height,
                rgba,
            } => self.register_image(layer, width, height, &rgba),
            ExternalUpdate::Release { layer } => self.release_image(layer),
            ExternalUpdate::Transition(settings) => self.begin_transition(settings),
            ExternalUpdate::TextRaster(mode) => self.set_text_raster(mode),
        });
    }

    pub fn begin_transition(&mut self, settings: Settings) {
        self.begun = Some(settings);
    }

    pub fn set_text_raster(&mut self, mode: TextRasterMode) {
        self.outline_text = mode == TextRasterMode::Outline;
    }

    pub fn transition_active(&self) -> bool {
        self.transition.is_some() || self.begun.is_some()
    }

    fn adopt_previous(&mut self, previous: &[PaintCommand]) {
        if let Some(settings) = self.begun.take() {
            self.transition = Some(Transition::begin(previous.to_vec(), settings));
        }
    }

    fn overlay_transition(
        &mut self,
        out: &mut Vec<PaintCommand>,
        surface: Rect,
        now: Instant,
    ) -> bool {
        let phase = self
            .transition
            .as_mut()
            .map(|transition| transition.advance(now));
        match (phase, self.transition.as_ref()) {
            (Some(Phase::Running(progress)), Some(transition)) => {
                transition.overlay(surface, progress, out);
                true
            }
            (Some(Phase::Finished), _) => {
                self.transition = None;
                false
            }
            _ => false,
        }
    }
}

fn lift_commands(commands: &mut [PaintCommand], lift: Transform2D) -> u32 {
    let linear = Linear2::of(lift);
    let scale = linear.scale_factor();
    let place = |bounds: Rect| {
        let center = lift.apply(bounds.center());
        Rect {
            x: center.x - bounds.w * 0.5,
            y: center.y - bounds.h * 0.5,
            ..bounds
        }
    };
    let mut approximated = 0;
    for command in commands.iter_mut() {
        let replacement = match command {
            PaintCommand::Quad(quad) => Some(PaintCommand::Linear {
                linear,
                leaf: Box::new(PaintCommand::Quad(Quad {
                    bounds: place(quad.bounds),
                    ..*quad
                })),
            }),
            PaintCommand::GradientQuad(item) => {
                let moved = place(item.quad.bounds);
                let (dx, dy) = (moved.x - item.quad.bounds.x, moved.y - item.quad.bounds.y);
                Some(PaintCommand::Linear {
                    linear,
                    leaf: Box::new(PaintCommand::GradientQuad(GradientQuad {
                        quad: Quad {
                            bounds: moved,
                            ..item.quad
                        },
                        gradient: Gradient {
                            start: [item.gradient.start[0] + dx, item.gradient.start[1] + dy],
                            end: [item.gradient.end[0] + dx, item.gradient.end[1] + dy],
                            ..item.gradient
                        },
                    })),
                })
            }
            PaintCommand::Image(image) => Some(PaintCommand::Linear {
                linear,
                leaf: Box::new(PaintCommand::Image(Image {
                    bounds: place(image.bounds),
                    ..*image
                })),
            }),
            PaintCommand::Shadow(shadow) => {
                approximated += 1;
                let offset = linear.apply(shadow.offset);
                Some(PaintCommand::Shadow(Shadow {
                    bounds: Rect::transform(shadow.bounds, lift),
                    radius: shadow.radius * scale,
                    offset,
                    sigma: shadow.sigma * scale,
                    spread: shadow.spread * scale,
                    ..*shadow
                }))
            }
            PaintCommand::PushClip(rect) => {
                approximated += 1;
                Some(PaintCommand::PushClip(Rect::transform(*rect, lift)))
            }
            PaintCommand::PushRoundedClip { rect, radius } => {
                approximated += 1;
                Some(PaintCommand::PushRoundedClip {
                    rect: Rect::transform(*rect, lift),
                    radius: *radius * scale,
                })
            }
            PaintCommand::PushContentBlur { bounds, radius } => {
                approximated += 1;
                Some(PaintCommand::PushContentBlur {
                    bounds: Rect::transform(*bounds, lift),
                    radius: *radius * scale,
                })
            }
            PaintCommand::BackdropBlur(blur) => {
                approximated += 1;
                Some(PaintCommand::BackdropBlur(BackdropBlur {
                    bounds: Rect::transform(blur.bounds, lift),
                    radius: blur.radius * scale,
                    corner_radius: blur.corner_radius * scale,
                }))
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            *command = replacement;
        }
    }
    approximated
}

fn glyph_is_plain(glyph: &aqua_text::layout::LaidGlyph) -> bool {
    glyph.color_srgb.is_none() && glyph.color_hsla.is_none() && glyph.source_color_hsla.is_none()
}

fn record_lowering(stats: &RenderStats, report: &LowerReport, elapsed: std::time::Duration) {
    let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
    stats.lower_ns.fetch_add(nanos, Ordering::Relaxed);
    stats.lower_max_ns.fetch_max(nanos, Ordering::Relaxed);
    stats
        .skipped_items
        .fetch_add(u64::from(report.unsupported()), Ordering::Relaxed);
    stats
        .commands
        .fetch_add(u64::from(report.commands), Ordering::Relaxed);
    stats
        .glyphs
        .fetch_add(u64::from(report.glyphs), Ordering::Relaxed);
}

impl aqua_transport::Lowering for Lowering {
    fn lower(&mut self, scene: &Scene, context: &LowerContext) -> Commands {
        let (commands, uploads, report) =
            self.commands(scene, context.size, context.scale, context.now);
        Commands {
            commands,
            uploads,
            unsupported: report.unsupported(),
            approximated: report.approximated,
            glyphs: report.glyphs,
        }
    }

    fn sync(&mut self) {
        self.drain_externals();
    }

    fn device_recovered(&mut self) {
        if self.recovery.consume() {
            self.reset();
        }
    }

    fn trim(&mut self, request: TrimRequest) -> usize {
        match request {
            TrimRequest::IdleFrames(frames) => self.residency.trim_idle(frames),
            TrimRequest::Everything => {
                self.slug_store.reset();
                self.slug_cache.clear();
                self.residency.trim_idle(0)
            }
        }
    }
}
