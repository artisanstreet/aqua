use aqua_core::hash::FastMap;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant as WallInstant;

use aqua_core::geometry::{CornerRadii, Rect, snap_device_pixel, snap_device_stroke};
use aqua_core::image::ImagePlacement;
use aqua_core::scene::{DisplayItem, ShadowLayer, TextRasterMode};
use aqua_core::{GpuiBackground, GpuiHsla, Oklch, Point, Size, Transform2D};
use aqua_graph::{
    BackdropBlur, Extent, FramePlan, Glyph, GlyphRun, Gradient, GradientQuad, GradientStop, Image,
    Linear2, PaintCommand, PlanError, PlanLimits, Quad, Shadow, SlugInstance, SlugRun, ToneCurve,
    plan_paint_with,
};
use aqua_media::atlas::{AtlasConfig, ImageKey, PageId, Placement, Slot, StreamBudget};
use aqua_media::reduce::{reduce, reduction_level};
use aqua_media::residency::{GlyphSlot, Residency, ResidencyConfig, VectorMode};
use aqua_media::upload::texture_handle;
use aqua_motion::{Cycle, CycleSet};
use aqua_protocol::{Instant, UploadBatch};
use aqua_scene::Scene;
use aqua_text::layout::LaidOut;
use aqua_text::raster::{SUBPIXEL_STEPS, quantize_size, quantize_x, raster_key};
use aqua_text::residency::GlyphResolver;
use aqua_text::runs::RunId;
use aqua_text::slug::{STORE_HANDLE, SlugCache, SlugStore, StoreLookup};
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
use super::vector::{VectorCache, gradient_axis, rotation_steps};

const MESH_TAG: u64 = 0xC000_0000_0000_0000;
const SLUG_TEXT_PX: f32 = 160.0;
const CULL_EDGE_PAD: f32 = 2.0;
const STILL_MARGIN_MICROS: u64 = 1_000;
const TEXT_OVERHANG_PAD: f32 = 6.0;
const TEXT_OVERHANG_RATIO: f32 = 0.15;
const RUN_CACHE_BYTES: usize = 3 << 20;

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
    pub atlas_created: u32,
    pub atlas_upload_bytes: u64,
    pub glyph_rasterized: u32,
    pub slug_created: u32,
    pub slug_upload_bytes: u64,
    pub culled: u32,
    pub pulsing: bool,
    pub shimmering: bool,
    pub still_micros: Option<u64>,
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

#[derive(Clone, Copy)]
struct PenY {
    relative: f32,
    bearing: i32,
}

impl PenY {
    fn top(self, base_y: f32) -> f32 {
        ((base_y + self.relative).round() as i32 - self.bearing) as f32
    }
}

struct RunEntry {
    signature: [u32; 11],
    laid: Arc<LaidOut>,
    epoch: u64,
    base: (f32, f32),
    commands: Vec<PaintCommand>,
    pens: Vec<Vec<PenY>>,
    pages: Vec<PageId>,
    glyphs: u32,
    bytes: usize,
}

#[derive(Default)]
struct RunCache {
    entries: FastMap<u64, RunEntry>,
    bytes: usize,
}

impl RunCache {
    fn insert(&mut self, run: u64, entry: RunEntry) {
        if self.bytes + entry.bytes > RUN_CACHE_BYTES {
            self.clear();
        }
        self.bytes += entry.bytes;
        if let Some(old) = self.entries.insert(run, entry) {
            self.bytes -= old.bytes;
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

fn kind_name(item: &DisplayItem) -> &'static str {
    match item {
        DisplayItem::Rect { .. } => "Rect",
        DisplayItem::Quad { .. } => "Quad",
        DisplayItem::Text { .. } => "Text",
        DisplayItem::Image { .. } => "Image",
        DisplayItem::External { .. } => "External",
        DisplayItem::Vector { .. } => "Vector",
        DisplayItem::GpuiSvg { .. } => "GpuiSvg",
        DisplayItem::GpuiTransformedSvg { .. } => "GpuiTransformedSvg",
        DisplayItem::GpuiPath { .. } => "GpuiPath",
        DisplayItem::GpuiMesh { .. } => "GpuiMesh",
        DisplayItem::GpuiShadow { .. } | DisplayItem::Shadow { .. } => "Shadow",
        DisplayItem::GpuiBackground { .. } => "GpuiBackground",
        DisplayItem::GpuiBorder { .. } => "GpuiBorder",
        DisplayItem::PushClip { .. } | DisplayItem::PushRoundedClip { .. } => "PushClip",
        DisplayItem::BackdropBlur { .. } | DisplayItem::BackdropBlurRounded { .. } => "Backdrop",
        DisplayItem::PushContentBlur { .. } => "ContentBlur",
        _ => "scope",
    }
}

fn replaced(command: &PaintCommand, pens: &[PenY], dx: f32, base_y: f32) -> PaintCommand {
    match (command, pens) {
        (PaintCommand::Glyphs(run), pens) => PaintCommand::Glyphs(GlyphRun {
            atlas: run.atlas,
            glyphs: run
                .glyphs
                .iter()
                .zip(pens)
                .map(|(glyph, pen)| Glyph {
                    bounds: Rect {
                        x: glyph.bounds.x + dx,
                        y: pen.top(base_y),
                        ..glyph.bounds
                    },
                    ..*glyph
                })
                .collect(),
        }),
        (PaintCommand::Image(image), [pen]) => PaintCommand::Image(Image {
            bounds: Rect {
                x: image.bounds.x + dx,
                y: pen.top(base_y),
                ..image.bounds
            },
            ..*image
        }),
        (other, _) => other.clone(),
    }
}

fn run_bytes(commands: &[PaintCommand]) -> usize {
    commands
        .iter()
        .map(|command| match command {
            PaintCommand::Glyphs(run) => {
                run.glyphs.len() * (std::mem::size_of::<Glyph>() + std::mem::size_of::<PenY>())
            }
            _ => std::mem::size_of::<PaintCommand>(),
        })
        .sum::<usize>()
        + std::mem::size_of::<RunEntry>()
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
    pulses: Vec<f32>,
    item: (u32, u32),
    lifted: bool,
    cull: Option<Rect>,
}

impl Walk {
    fn hidden(&mut self, rect: Rect, reach: f32) -> bool {
        let Some(region) = self.cull else {
            return false;
        };
        if self.lifted || self.open.contains(&Scope::Blur) {
            return false;
        }
        let pad = reach + CULL_EDGE_PAD;
        let outside = rect.x - pad >= region.right()
            || region.x >= rect.right() + pad
            || rect.y - pad >= region.bottom()
            || region.y >= rect.bottom() + pad;
        if outside {
            self.report.culled += 1;
        }
        outside
    }

    fn emit_box(
        &mut self,
        bounds: Rect,
        radii: Radii,
        paint: Option<Paint>,
        border: Option<Border>,
    ) {
        if !self.hidden(bounds, 0.0) {
            shape::emit_box(&mut self.out, bounds, radii, paint, border);
        }
    }

    fn emit_shadow(&mut self, bounds: Rect, radii: Radii, spec: ShadowSpec) -> Fidelity {
        let reach = spec.sigma.abs() * 3.0 + spec.spread.abs() + spec.dx.abs().max(spec.dy.abs());
        match self.hidden(bounds, reach) {
            true => Fidelity::Exact,
            false => shape::emit_shadow(&mut self.out, bounds, radii, spec),
        }
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
            Some(factor) => {
                self.report.pulsing = true;
                factor
            }
            None => 1.0,
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
    glyph_inserts: u64,
    retained: HashMap<u64, RetainedImage>,
    meshes: HashMap<u64, ImageKey>,
    reduced: HashMap<u64, Vec<ImageKey>>,
    vectors: VectorCache,
    text: TextStack,
    run_cache: RunCache,
    shimmer: ShimmerClocks,
    pulse_cycles: CycleSet<(u32, u32)>,
    externals: Option<Receiver<ExternalUpdate>>,
    recovery: RecoverySignal,
    stats: Option<Arc<RenderStats>>,
    report_pages: u32,
    clear: Oklch,
    last_now: Instant,
    last_report: LowerReport,
    cull: Option<Rect>,
    last_commands: Vec<PaintCommand>,
    spare_commands: Vec<PaintCommand>,
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
            glyph_inserts: 0,
            retained: HashMap::new(),
            meshes: HashMap::new(),
            reduced: HashMap::new(),
            vectors: VectorCache::default(),
            text: TextStack::default(),
            run_cache: RunCache::default(),
            shimmer: ShimmerClocks::default(),
            pulse_cycles: CycleSet::new(),
            externals: None,
            recovery: RecoverySignal::default(),
            stats: None,
            report_pages: 0,
            clear: Oklch::TRANSPARENT,
            last_now: Instant::ZERO,
            last_report: LowerReport::default(),
            cull: None,
            last_commands: Vec::new(),
            spare_commands: Vec::new(),
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
        self.reduced
            .remove(&layer)
            .into_iter()
            .flatten()
            .for_each(|key| {
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
        self.run_cache.clear();
    }

    pub fn trim(&mut self, idle_frames: u64) -> usize {
        self.residency.trim_idle(idle_frames)
    }

    pub fn residency(&self) -> &Residency {
        &self.residency
    }

    pub fn set_cull(&mut self, region: Option<Rect>) {
        self.cull = region;
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
        let mut buffer = std::mem::take(&mut self.spare_commands);
        buffer.clear();
        buffer.reserve(scene.len() * 2);
        self.residency.begin_frame();
        self.shimmer.begin_frame();
        self.pulse_cycles.begin_frame();
        self.text.clear();
        let extent = physical_extent(size, scale);
        let mut walk = Walk {
            base: Affine::device(scale),
            transforms: Vec::new(),
            open: Vec::new(),
            out: buffer,
            report: LowerReport::default(),
            surface: Rect {
                x: 0.0,
                y: 0.0,
                w: extent.width as f32,
                h: extent.height as f32,
            },
            now_secs: now.as_secs_f64(),
            pulses: Vec::new(),
            item: (0, 0),
            lifted: false,
            cull: match (self.begun.is_some(), self.transition.is_some()) {
                (false, false) => self.cull,
                _ => None,
            },
        };
        let profiled = crate::profile::enabled();
        let mut kinds: std::collections::BTreeMap<&'static str, (u32, u64)> = Default::default();
        let loop_started = WallInstant::now();
        for (id, item) in scene.ordered() {
            walk.report.items += 1;
            walk.item = (id.raw().index, id.raw().generation);
            match profiled {
                true => {
                    let item_started = WallInstant::now();
                    self.emit(item, &mut walk);
                    let spent = item_started.elapsed().as_nanos() as u64;
                    let slot = kinds.entry(kind_name(item)).or_insert((0, 0));
                    slot.0 += 1;
                    slot.1 += spent;
                }
                false => self.emit(item, &mut walk),
            }
        }
        let loop_us = loop_started.elapsed().as_micros();
        while let Some(kind) = walk.open.pop() {
            walk.out.push(match kind {
                Scope::Clip => PaintCommand::PopClip,
                Scope::Opacity => PaintCommand::PopOpacity,
                Scope::Blur => PaintCommand::PopContentBlur,
            });
        }
        walk.report.shimmering = self.shimmer.active();
        walk.report.still_micros = self
            .shimmer
            .hold()
            .map(|seconds| (seconds * 1.0e6).ceil() as u64 + STILL_MARGIN_MICROS);
        self.shimmer.end_frame();
        self.pulse_cycles.end_frame();
        self.adopt_previous(&previous);
        if self.transition.is_some() {
            self.last_commands.clone_from(&walk.out);
        }
        previous.clear();
        self.spare_commands = previous;
        if let Some(region) = self.cull {
            walk.report.culled += super::cull::cull(&mut walk.out, region);
            if let Some(stats) = &self.stats {
                stats
                    .culled_commands
                    .fetch_add(u64::from(walk.report.culled), Ordering::Relaxed);
            }
        }
        crate::profile::note(|| {
            let mut top: Vec<_> = kinds.iter().collect();
            top.sort_by_key(|(_, (_, spent))| std::cmp::Reverse(*spent));
            let top: Vec<String> = top
                .iter()
                .take(6)
                .map(|(name, (count, spent))| format!("{name}:{count}:{}us", spent / 1000))
                .collect();
            format!(
                "walk_kinds=[{}] walk_items={} walk_culled={} walk_us={} loop_us={loop_us} pre_us={}",
                top.join(","),
                walk.report.items,
                walk.report.culled,
                started.elapsed().as_micros(),
                loop_started.duration_since(started).as_micros()
            )
        });
        walk.report.shimmering |= self.overlay_transition(&mut walk.out, walk.surface, now);
        walk.report.commands = walk.out.len() as u32;
        let inserts_before = self.glyph_inserts;
        let updates = self.residency.finish_frame();
        walk.report.atlas_created = updates.created.len() as u32;
        walk.report.atlas_upload_bytes = updates.upload_bytes();
        let resident_now = self.residency.stats();
        self.glyph_inserts = resident_now.masks.inserts + resident_now.colors.inserts;
        walk.report.glyph_rasterized =
            (self.glyph_inserts - inserts_before.min(self.glyph_inserts)) as u32;
        let mut uploads = aqua_media::upload::upload_batch(updates);
        let slug_uploads = self.slug_store.uploads();
        for upload in &slug_uploads.textures {
            match &upload.op {
                aqua_protocol::TextureOp::Create { .. } => walk.report.slug_created += 1,
                aqua_protocol::TextureOp::Write { data, .. } => {
                    walk.report.slug_upload_bytes += data.len() as u64;
                }
                aqua_protocol::TextureOp::Release => {}
            }
        }
        uploads.textures.extend(slug_uploads.textures);
        if let Some(stats) = &self.stats {
            record_lowering(stats, &walk.report, started.elapsed());
            let resident = self.residency.stats();
            let report_signature =
                resident.colors.pages + (self.slug_store.resident_bytes() / 16384) as u32 * 64;
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
                    self.retained
                        .values()
                        .map(|image| image.premultiplied.len())
                        .sum::<usize>(),
                    resident.streaming_bytes,
                );
            }
            stats
                .slug_bytes
                .store(self.slug_store.resident_bytes(), Ordering::Relaxed);
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
        let planned = plan_paint_with(physical_extent(size, scale), self.clear, &commands, limits);
        self.retain_frame(commands);
        let plan = planned?;
        Ok(Lowered {
            uploads,
            plan: FramePlan {
                text_tone: self.text_tone(),
                ..plan
            },
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
                    let fidelity = walk.emit_shadow(bounds, radii, spec);
                    walk.note(fidelity);
                }
                let border = Border {
                    widths: Sides::uniform(border_width * s),
                    color: *border_color,
                    dashed: false,
                };
                walk.emit_box(bounds, radii, Some(Paint::Solid(*fill)), Some(border));
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
                walk.emit_box(bounds, radii, Some(paint), Some(border));
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
                walk.emit_box(bounds, radii, Some(paint), None);
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
                walk.emit_box(bounds, radii, None, Some(border));
            }
            DisplayItem::Shadow {
                bounds,
                corners,
                layer,
            } => {
                let fidelity = walk.emit_shadow(
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
                let fidelity = walk.emit_shadow(
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
                let destination = at.rect(*bounds);
                if walk.hidden(destination, 0.0) {
                    return;
                }
                let vector = self.vectors.svg(source, tint.map(hsla_oklch), 0);
                self.emit_vector(vector, destination, *opacity, walk);
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
                let destination = at.rect(placed);
                if walk.hidden(destination, destination.w.max(destination.h)) {
                    return;
                }
                let vector = self.vectors.svg(
                    source,
                    Some(hsla_oklch(*tint)),
                    rotation_steps(transform.rotation_radians),
                );
                self.emit_vector(vector, destination, *opacity, walk);
            }
            DisplayItem::GpuiPath {
                path,
                transformed_window_logical_bounds,
            } => {
                if !walk.hidden(at.rect(*transformed_window_logical_bounds), 1.0) {
                    self.emit_path(path, at, walk)
                }
            }
            DisplayItem::GpuiMesh {
                bounds,
                corners,
                mesh,
                style,
            } => self.emit_mesh(snapped(at.rect(*bounds)), *corners, mesh, style, s, walk),
            DisplayItem::PushGpuiQuadPulse(pulse) => {
                let cycle = Cycle::Pulse {
                    period: f64::from(pulse.period_ms) / 1000.0,
                    floor: pulse.min_opacity,
                };
                let factor = self.pulse_cycles.sample(walk.item, cycle, walk.now_secs);
                walk.pulses.push(factor)
            }
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
        walk.open.push(Scope::Clip);
        match radius >= 0.75 {
            true => {
                walk.note(match fitted.is_uniform() {
                    true => Fidelity::Exact,
                    false => Fidelity::Approximated,
                });
                walk.out
                    .push(PaintCommand::PushRoundedClip { rect, radius });
            }
            false => walk.out.push(PaintCommand::PushClip(rect)),
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
        if walk.cull.is_some() && at.is_exact() {
            let pad = (TEXT_OVERHANG_PAD + TEXT_OVERHANG_RATIO * laid.size.1) * s;
            let ink = Rect {
                x: base_x,
                y: base_y,
                w: laid.size.0 * s,
                h: laid.size.1 * s,
            };
            if walk.hidden(ink, pad) {
                if let Some(entry) = self.run_cache.entries.get(&run_id) {
                    for page in &entry.pages {
                        self.residency.touch_page(*page);
                    }
                }
                return;
            }
        }
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
        let mut batches: Vec<(aqua_media::atlas::PageId, Vec<Glyph>, Vec<PenY>)> = Vec::new();
        let mut color_images: Vec<PaintCommand> = Vec::new();
        let mut color_pens: Vec<PenY> = Vec::new();
        let mut last_color: Option<([f32; 4], Oklch)> = None;
        let moving = !walk.transforms.is_empty()
            && (!at.is_exact() || (s / walk.base.scale() - 1.0).abs() > 1.0e-3);
        let mut curves: Vec<SlugInstance> = Vec::new();
        let signature = self
            .text
            .cache_signature()
            .filter(|_| !moving && !self.outline_text && phase == 0.0)
            .map(|text| {
                [
                    (base_x - base_x.floor()).to_bits(),
                    s.to_bits(),
                    scene_color.l.to_bits(),
                    scene_color.c.to_bits(),
                    scene_color.h.to_bits(),
                    scene_color.alpha.to_bits(),
                    opacity.to_bits(),
                    text[0] ^ (text[1] << 1),
                    text[2],
                    text[3],
                    text[4],
                ]
            });
        let epoch = self.residency.eviction_epoch();
        if let Some(signature) = signature {
            if let Some(entry) = self.run_cache.entries.get(&run_id).filter(|entry| {
                entry.signature == signature
                    && entry.epoch == epoch
                    && Arc::ptr_eq(&entry.laid, &laid)
            }) {
                match entry.base == (base_x, base_y) {
                    true => walk.out.extend(entry.commands.iter().cloned()),
                    false => {
                        let dx = base_x.floor() - entry.base.0.floor();
                        walk.out.extend(
                            entry
                                .commands
                                .iter()
                                .zip(&entry.pens)
                                .map(|(command, pens)| replaced(command, pens, dx, base_y)),
                        );
                    }
                }
                walk.report.glyphs += entry.glyphs;
                for page in &entry.pages {
                    self.residency.touch_page(*page);
                }
                return;
            }
        }
        let started = walk.out.len();
        let glyphs_before = walk.report.glyphs;
        let skipped_before = walk.report.unsupported();
        let mut pages: Vec<PageId> = Vec::new();
        let mut curved = false;
        let fonts_revision = self.fonts.revision();
        for line in &laid.lines {
            for glyph in &line.glyphs {
                if moving || self.outline_text || glyph.font_size_px * s >= SLUG_TEXT_PX {
                    curved = true;
                    let key = raster_key(glyph, 0);
                    let looked = match self.slug_store.resident(&key, fonts_revision) {
                        Some(found) => found,
                        None => {
                            let mut fonts = self.fonts.lock();
                            self.slug_store
                                .lookup(&mut self.slug_cache, &mut fonts, &key)
                        }
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
                let relative_y = (line.baseline_y + glyph.y - glyph.y_offset) * s;
                let pen_y = (base_y + relative_y).round() as i32;
                let mut key = raster_key(glyph, subpixel);
                key.size_bits = quantize_size(glyph.font_size_px * s).to_bits();
                let resolved = self
                    .resolver
                    .resolve_shared(&mut self.residency, &self.fonts, &key);
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
                if !pages.contains(&placement.page) {
                    pages.push(placement.page);
                }
                let glyph_x = match slot {
                    GlyphSlot::Color(_) => pixel_x + i32::from(subpixel >= SUBPIXEL_STEPS / 2),
                    GlyphSlot::Mask(_) => pixel_x,
                };
                let bounds = Rect {
                    x: (glyph_x + placement.bearing[0]) as f32,
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
                        };
                        let pen = PenY {
                            relative: relative_y,
                            bearing: placement.bearing[1],
                        };
                        match batches.last_mut() {
                            Some((page, glyphs, pens)) if *page == placement.page => {
                                glyphs.push(entry);
                                pens.push(pen);
                            }
                            _ => batches.push((placement.page, vec![entry], vec![pen])),
                        }
                    }
                    GlyphSlot::Color(_) => {
                        color_pens.push(PenY {
                            relative: relative_y,
                            bearing: placement.bearing[1],
                        });
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
        let mut run_pens: Vec<Vec<PenY>> = Vec::new();
        for (page, glyphs, pens) in batches {
            walk.out.push(PaintCommand::Glyphs(GlyphRun {
                atlas: texture_handle(page),
                glyphs: glyphs.into(),
            }));
            run_pens.push(pens);
        }
        if !curves.is_empty() {
            walk.out.push(PaintCommand::Slug(SlugRun {
                store: STORE_HANDLE,
                glyphs: curves,
            }));
        }
        walk.out.extend(color_images);
        if let Some(signature) = signature.filter(|_| {
            !curved
                && walk.report.unsupported() == skipped_before
                && self.residency.eviction_epoch() == epoch
        }) {
            let commands = walk.out[started..].to_vec();
            let mut color_pens = color_pens.into_iter();
            let mut run_pens = run_pens.into_iter();
            let pens: Vec<Vec<PenY>> = commands
                .iter()
                .map(|command| match command {
                    PaintCommand::Glyphs(_) => run_pens.next().unwrap_or_default(),
                    PaintCommand::Image(_) => color_pens.next().into_iter().collect(),
                    _ => Vec::new(),
                })
                .collect();
            self.run_cache.insert(
                run_id,
                RunEntry {
                    signature,
                    laid: laid.clone(),
                    epoch,
                    base: (base_x, base_y),
                    bytes: run_bytes(&commands),
                    commands,
                    pens,
                    pages,
                    glyphs: walk.report.glyphs - glyphs_before,
                },
            );
        }
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
        if walk.hidden(bounds, 0.0) {
            return;
        }
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

    fn text_tone(&self) -> ToneCurve {
        match self.outline_text {
            true => ToneCurve::EXACT,
            false => ToneCurve::native(),
        }
    }

    pub fn transition_active(&self) -> bool {
        self.transition.is_some() || self.begun.is_some()
    }

    pub fn retain_frame(&mut self, commands: Vec<PaintCommand>) {
        if self.transition.is_none() {
            self.last_commands = commands;
        }
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
    stats
        .atlas_created
        .fetch_add(u64::from(report.atlas_created), Ordering::Relaxed);
    stats
        .atlas_upload_bytes
        .fetch_add(report.atlas_upload_bytes, Ordering::Relaxed);
    stats
        .glyph_rasterized
        .fetch_add(u64::from(report.glyph_rasterized), Ordering::Relaxed);
    stats
        .slug_created
        .fetch_add(u64::from(report.slug_created), Ordering::Relaxed);
    stats
        .slug_upload_bytes
        .fetch_add(report.slug_upload_bytes, Ordering::Relaxed);
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
            text_tone: self.text_tone(),
        }
    }

    fn sync(&mut self) {
        self.drain_externals();
    }

    fn reclaim(&mut self, commands: Vec<PaintCommand>) {
        self.retain_frame(commands);
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
