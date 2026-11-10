use aqua_core::hash::FastMap;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant as WallInstant;

use aqua_core::geometry::{snap_device_pixel, snap_device_stroke, CornerRadii, Rect};
use aqua_core::image::ImagePlacement;
use aqua_core::scene::{DisplayItem, ShadowLayer, TextRasterMode};
use aqua_core::{GpuiBackground, GpuiHsla, Oklch, Point, Size, Transform2D};
use aqua_graph::{
    plan_paint_with, BackdropBlur, Coverage, Extent, FramePlan, Glyph, GlyphRun, Gradient,
    GradientQuad, GradientStop, Image, Linear2, PaintCommand, PlanError, PlanLimits, Quad, Shadow,
    SlugInstance, SlugRun, ToneCurve,
};
use aqua_media::atlas::{AtlasConfig, ImageKey, PageId, Placement, Slot, StreamBudget};
use aqua_media::reduce::{reduce, reduction_level};
use aqua_media::residency::{GlyphSlot, Residency, ResidencyConfig, VectorKey, VectorMode};
use aqua_media::upload::texture_handle;
use aqua_motion::{Cycle, CycleSet};
use aqua_protocol::{Instant, UploadBatch};
use aqua_scene::Scene;
use aqua_text::layout::LaidOut;
use aqua_text::raster::{quantize_size, quantize_x, raster_key, SUBPIXEL_STEPS};
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
use super::span::{self, SpanCache};
use super::text::{ShimmerClocks, TextScope, TextStack};
use super::transition::{Phase, Settings, Transition};
use super::vector::{gradient_axis, rotation_steps, VectorCache};

const MESH_TAG: u64 = 0xC000_0000_0000_0000;
const SLUG_TEXT_PX: f32 = 160.0;
const CULL_EDGE_PAD: f32 = 2.0;
const STILL_MARGIN_MICROS: u64 = 1_000;
const TEXT_OVERHANG_PAD: f32 = 6.0;
const TEXT_OVERHANG_RATIO: f32 = 0.15;
const RUN_CACHE_BYTES: usize = 3 << 20;
const PATH_STORE_HANDLE: aqua_graph::TextureHandle = aqua_graph::TextureHandle(0x00ff_fffd);

fn touch_curves(
    commands: &[PaintCommand],
    text: &mut SlugStore,
    paths: &mut SlugStore<u64>,
) -> bool {
    commands.iter().all(|command| match command {
        PaintCommand::Slug(run) if run.store == STORE_HANDLE => {
            run.glyphs.iter().all(|glyph| text.touch_index(glyph.glyph))
        }
        PaintCommand::Slug(run) if run.store == PATH_STORE_HANDLE => run
            .glyphs
            .iter()
            .all(|glyph| paths.touch_index(glyph.glyph)),
        PaintCommand::Linear { leaf, .. } => touch_curves(std::slice::from_ref(leaf), text, paths),
        _ => true,
    })
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
    pub cached_text_runs: u32,
    pub approximated: u32,
    pub pending: u32,
    pub atlas_created: u32,
    pub atlas_upload_bytes: u64,
    pub glyph_rasterized: u32,
    pub slug_created: u32,
    pub slug_upload_bytes: u64,
    pub culled: u32,
    pub pulsing: bool,
    pub shader_animation: bool,
    pub shape_animation: bool,
    pub shimmering: bool,
    pub still_micros: Option<u64>,
    pub skipped: BTreeMap<Unsupported, u32>,
}

impl LowerReport {
    #[must_use]
    pub fn animated(&self) -> bool {
        self.pulsing || self.shader_animation || self.shape_animation
    }

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
    signature: [u32; 12],
    laid: Arc<LaidOut>,
    stamp: span::Stamp,
    base: (f32, f32),
    commands: Vec<PaintCommand>,
    pens: Vec<Vec<PenY>>,
    pages: Vec<PageId>,
    glyphs: u32,
    curves: u32,
    bytes: usize,
}

#[derive(Default)]
struct RunCache {
    entries: FastMap<u64, RunEntry>,
    bytes: usize,
}

impl RunCache {
    fn insert(&mut self, run: u64, entry: RunEntry) {
        if entry.bytes > RUN_CACHE_BYTES {
            if let Some(previous) = self.entries.remove(&run) {
                self.bytes -= previous.bytes;
            }
            return;
        }
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

const VOLATILE_FRAMES: u64 = 30;

const SEGMENT_ITEMS: usize = 24;

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
        DisplayItem::Shader { .. } => "Shader",
        DisplayItem::Shape { .. } => "Shape",
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
            coverage: run.coverage,
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
            PaintCommand::Slug(run) => {
                run.glyphs.len() * std::mem::size_of::<SlugInstance>()
                    + std::mem::size_of::<PaintCommand>()
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

struct Marks {
    pending: u32,
    approximated: u32,
    unsupported: u32,
    glyphs: u32,
    curves: u32,
    pulsing: bool,
}

struct Recording {
    id: aqua_core::NodeId,
    segment: bool,
    open: usize,
    depth: usize,
    starts: Vec<u32>,
    contexts: Vec<Option<span::Context>>,
    matching: Vec<u32>,
    pending_scopes: Vec<(u32, Nesting)>,
    broken: bool,
    tainted: bool,
    laid_start: usize,
    out_start: usize,
    paths_start: usize,
    marks: Marks,
    environment: span::Environment,
    origin: (f32, f32),
    clip: Option<Rect>,
    stamp: span::Stamp,
}

struct Walk {
    recordings: Vec<Recording>,
    closed: bool,
    laid: Vec<(u64, Arc<LaidOut>)>,
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
    clips: Vec<Rect>,
}

fn outside_region(rect: Rect, region: Rect, pad: f32) -> bool {
    rect.x - pad >= region.right()
        || region.x >= rect.right() + pad
        || rect.y - pad >= region.bottom()
        || region.y >= rect.bottom() + pad
}

impl Walk {
    fn seeded(parent: &Walk, context: &span::Context) -> Self {
        Self {
            recordings: Vec::new(),
            closed: false,
            laid: Vec::new(),
            base: parent.base,
            transforms: vec![Affine(context.affine)],
            open: match context.blurred {
                true => vec![Scope::Blur],
                false => Vec::new(),
            },
            out: Vec::new(),
            report: LowerReport::default(),
            surface: parent.surface,
            now_secs: parent.now_secs,
            pulses: Vec::new(),
            item: (0, 0),
            lifted: false,
            cull: None,
            clips: context.clip.into_iter().collect(),
        }
    }

    fn hidden(&mut self, rect: Rect, reach: f32) -> bool {
        if self.lifted || self.open.contains(&Scope::Blur) {
            return false;
        }
        let pad = reach + CULL_EDGE_PAD;
        let outside = self
            .cull
            .into_iter()
            .chain(self.clips.last().copied())
            .any(|region| outside_region(rect, region, pad));
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

    fn open_clip(&mut self, rect: Rect) {
        let nested = self
            .clips
            .last()
            .map_or(rect, |outer| outer.intersect(rect));
        self.open.push(Scope::Clip);
        self.clips.push(nested);
    }

    fn close(&mut self, kind: Scope) -> bool {
        match self.open.last() {
            Some(top) if *top == kind => {
                self.open.pop();
                if kind == Scope::Clip {
                    self.clips.pop();
                }
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
    path_store: SlugStore<u64>,
    shapes: super::shape_motion::Shapes,
    shader_time: Option<Instant>,
    glyph_inserts: u64,
    retained: HashMap<u64, RetainedImage>,
    meshes: HashMap<u64, ImageKey>,
    path_keys: Vec<VectorKey>,
    path_keys_now: Vec<VectorKey>,
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
    spans: SpanCache,
    span_replays: u64,
    span_rejects: [u32; 8],
    span_misses: [u32; 10],
    span_trace: Vec<String>,
    volatile_spans: HashMap<aqua_core::NodeId, (u64, usize)>,
    span_frame: u64,
    slug_generation: u64,
    diag_seen: FastMap<aqua_core::NodeId, u64>,
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

fn path_style_label(path: &aqua_core::vector::GpuiPath) -> &'static str {
    use aqua_core::vector::GpuiPathStyle;
    match (&path.style, &path.background) {
        (GpuiPathStyle::Fill { .. }, aqua_core::GpuiBackground::Solid(_)) => "fill_solid",
        (GpuiPathStyle::Fill { .. }, _) => "fill_gradient",
        (_, aqua_core::GpuiBackground::Solid(_)) => "stroke_solid",
        _ => "stroke_gradient",
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
            path_store: SlugStore::with_budget(8 << 20),
            shapes: super::shape_motion::Shapes::default(),
            shader_time: None,
            glyph_inserts: 0,
            retained: HashMap::new(),
            meshes: HashMap::new(),
            path_keys: Vec::new(),
            path_keys_now: Vec::new(),
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
            spans: SpanCache::default(),
            span_replays: 0,
            span_rejects: [0; 8],
            span_misses: [0; 10],
            span_trace: Vec::new(),
            volatile_spans: HashMap::new(),
            span_frame: 0,
            slug_generation: 0,
            diag_seen: FastMap::default(),
        }
    }

    #[must_use]
    pub fn with_frozen_shaders(mut self, now: Instant) -> Self {
        self.shader_time = Some(now);
        self
    }

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
        self.spans.clear();
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
        self.path_store.reset();
        self.shapes.clear();
        self.meshes.clear();
        self.reduced.clear();
        self.vectors.clear();
        self.run_cache.clear();
        self.spans.clear();
        self.volatile_spans.clear();
        self.slug_generation += 1;
    }

    pub fn trim(&mut self, idle_frames: u64) -> usize {
        self.spans.clear();
        self.residency.trim_idle(idle_frames)
    }

    #[must_use]
    pub fn span_replays(&self) -> u64 {
        self.span_replays
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
        self.drain_externals();
        let started = WallInstant::now();
        self.last_now = now;
        self.span_trace.clear();
        self.span_frame += 1;
        let mut previous = std::mem::take(&mut self.last_commands);
        let mut buffer = std::mem::take(&mut self.spare_commands);
        buffer.clear();
        buffer.reserve(scene.len() * 2);
        self.residency.begin_frame();
        self.shimmer.begin_frame();
        self.pulse_cycles.begin_frame();
        self.shapes.begin_frame();
        self.text.clear();
        let extent = physical_extent(size, scale);
        let mut walk = Walk {
            recordings: Vec::new(),
            closed: false,
            laid: Vec::new(),
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
            clips: Vec::new(),
            cull: match (self.begun.is_some(), self.transition.is_some()) {
                (false, false) => self.cull,
                _ => None,
            },
        };
        let profiled = crate::profile::enabled();
        let mut kinds: std::collections::BTreeMap<&'static str, (u32, u64)> = Default::default();
        if profiled {
            let mut changed: BTreeMap<&'static str, u32> = Default::default();
            let mut samples: Vec<String> = Vec::new();
            let mut now_seen: FastMap<aqua_core::NodeId, u64> = FastMap::default();
            for (index, (id, item)) in scene.ordered().enumerate() {
                let revision = scene.item_revision(id).unwrap_or(0);
                now_seen.insert(id, revision);
                if self.diag_seen.get(&id) != Some(&revision) {
                    *changed.entry(kind_name(item)).or_insert(0) += 1;
                    if samples.len() < 8 && kind_name(item) != "scope" {
                        let text = format!("{item:?}");
                        samples.push(format!(
                            "{index}:{}",
                            text.chars().take(110).collect::<String>()
                        ));
                    }
                }
            }
            self.diag_seen = now_seen;
            crate::profile::note(|| format!("diag_changed={changed:?} diag_samples={samples:?}"));
        }
        let loop_started = WallInstant::now();
        let total = scene.len();
        let mut position = 0;
        let mut replayed = 0u32;
        let mut hold_until = 0usize;
        let mut emitted_ranges: Vec<(usize, usize)> = Vec::new();
        let mut recorded = 0u32;
        while position < total {
            let Some((id, item)) = scene.item_at(position) else {
                break;
            };
            let transform = match item {
                DisplayItem::PushTransform(local) => Some(local),
                _ => None,
            };
            let mut segment_start =
                transform.is_none() && position >= hold_until && self.at_baseline(&walk);
            if segment_start {
                if let Some((seen, len)) = self.volatile_spans.get(&id).copied() {
                    if self.span_frame - seen < VOLATILE_FRAMES {
                        hold_until = position + len + 1;
                        segment_start = false;
                    }
                }
            }
            if transform.is_some() || segment_start {
                let before = walk.out.len() as u32;
                if let Some(close) = self.replay_span(scene, position, id, transform, &mut walk) {
                    replayed += 1;
                    let after = walk.out.len() as u32;
                    for recording in walk.recordings.iter_mut() {
                        recording.starts.push(before);
                        recording.contexts.push(None);
                        recording.matching.push(span::NONE);
                        for _ in position + 1..=close {
                            recording.starts.push(after);
                            recording.contexts.push(None);
                            recording.matching.push(span::NONE);
                        }
                    }
                    position = close + 1;
                    continue;
                }
                if self.begin_span(position, id, transform, &mut walk) {
                    recorded += 1;
                }
            }
            if profiled {
                match emitted_ranges.last_mut() {
                    Some((_, end)) if *end + 1 == position => *end = position,
                    _ => emitted_ranges.push((position, position)),
                }
            }
            self.note_item(item, &mut walk);
            walk.report.items += 1;
            walk.item = (id.raw().index, id.raw().generation);
            if taints_span(item) {
                walk.recordings
                    .iter_mut()
                    .for_each(|recording| recording.tainted = true);
            }
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
            if walk.closed {
                walk.closed = false;
                self.finish_span(scene, position, &mut walk);
            } else if walk.recordings.last().is_some_and(|recording| {
                recording.segment && position + 1 - recording.open >= SEGMENT_ITEMS
            }) && self.at_baseline_inside(&walk)
            {
                self.finish_span(scene, position, &mut walk);
            }
            position += 1;
        }
        walk.recordings.clear();
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
        self.shapes.end_frame();
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
                "walk_kinds=[{}] walk_items={} walk_culled={} text_cached={} walk_us={} loop_us={loop_us} pre_us={} spans=replayed:{replayed},recorded:{recorded},cached:{},rejects:{:?},misses:{:?}",
                top.join(","),
                walk.report.items,
                walk.report.culled,
                walk.report.cached_text_runs,
                started.elapsed().as_micros(),
                loop_started.duration_since(started).as_micros(),
                self.spans.len(),
                self.span_rejects,
                self.span_misses
            )
        });
        crate::profile::note(|| {
            format!(
                "span_trace={:?} emitted_ranges={emitted_ranges:?}",
                self.span_trace
            )
        });
        walk.report.shimmering |= self.overlay_transition(&mut walk.out, walk.surface, now);
        walk.report.commands = walk.out.len() as u32;
        let inserts_before = self.glyph_inserts;
        self.retire_reshaped_paths();
        let updates = self.residency.finish_frame();
        walk.report.atlas_created = updates.created.len() as u32;
        walk.report.atlas_upload_bytes = updates.upload_bytes();
        let resident_now = self.residency.stats();
        self.glyph_inserts = resident_now.masks.inserts + resident_now.colors.inserts;
        walk.report.glyph_rasterized =
            (self.glyph_inserts - inserts_before.min(self.glyph_inserts)) as u32;
        let mut uploads = aqua_media::upload::upload_batch(updates);
        let mut slug_uploads = self.slug_store.uploads();
        let mut path_uploads = self.path_store.uploads();
        for upload in &mut path_uploads.textures {
            upload.handle = PATH_STORE_HANDLE;
        }
        slug_uploads.textures.extend(path_uploads.textures);
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
            let report_signature = resident.colors.pages
                + ((self.slug_store.resident_bytes() + self.path_store.resident_bytes()) / 16384)
                    as u32
                    * 64;
            if self.report_pages != report_signature && crate::mem_probe::memory_report_enabled() {
                self.report_pages = report_signature;
                eprintln!("aqua-atlas-pages: {:?}", self.residency.page_layouts());
                eprintln!("aqua-atlas-keys: {:?}", self.residency.page_keys());
                eprintln!(
                    "aqua-lowering-heap: slug_cache={} slug_store_cpu={} slug_store_gpu={} retained_vectors={} retained_images={} streaming={}",
                    self.slug_cache.heap_bytes(),
                    (self.slug_store.texel_count() + self.path_store.texel_count()) * 16,
                    self.slug_store.resident_bytes() + self.path_store.resident_bytes(),
                    resident.retained_source_bytes,
                    self.retained
                        .values()
                        .map(|image| image.premultiplied.len())
                        .sum::<usize>(),
                    resident.streaming_bytes,
                );
            }
            stats.slug_bytes.store(
                self.slug_store.resident_bytes() + self.path_store.resident_bytes(),
                Ordering::Relaxed,
            );
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

    fn span_stamp(&self) -> span::Stamp {
        span::Stamp {
            epoch: self.residency.eviction_epoch(),
            fonts: self.fonts.revision(),
            slug: self
                .slug_store
                .evictions()
                .wrapping_add(self.path_store.evictions())
                .wrapping_add(self.slug_generation << 32),
        }
    }

    fn span_environment(walk: &Walk) -> span::Environment {
        let parent = walk.at().0;
        span::Environment {
            scale: walk.base.scale().to_bits(),
            surface: (walk.surface.w.to_bits(), walk.surface.h.to_bits()),
            clip: walk.clips.last().copied().map(span::clip_bits),
            open: walk.open.len(),
            blurred: walk.open.contains(&Scope::Blur),
            parent: [
                parent.a.to_bits(),
                parent.b.to_bits(),
                parent.c.to_bits(),
                parent.d.to_bits(),
            ],
        }
    }

    fn span_origin(&mut self, walk: &Walk, local: &Transform2D) -> Option<(f32, f32)> {
        let at = walk.at();
        let reasons = [
            walk.lifted,
            walk.cull.is_some(),
            !walk.pulses.is_empty(),
            !self.text.is_balanced(),
            self.transition.is_some() || self.begun.is_some(),
            !is_translation(local),
            at.0.b != 0.0 || at.0.c != 0.0,
        ];
        if let Some(index) = reasons.iter().position(|rejected| *rejected) {
            self.span_rejects[index] += 1;
            return None;
        }
        let next = snapped_nest(walk, at, local);
        next.finite().then_some((next.0.tx, next.0.ty))
    }

    fn at_baseline(&self, walk: &Walk) -> bool {
        walk.recordings.is_empty() && self.at_baseline_inside(walk)
    }

    fn at_baseline_inside(&self, walk: &Walk) -> bool {
        walk.transforms.is_empty()
            && walk.open.is_empty()
            && walk.clips.is_empty()
            && walk.pulses.is_empty()
            && !walk.lifted
            && walk.cull.is_none()
            && self.text.is_balanced()
            && self.transition.is_none()
            && self.begun.is_none()
    }

    fn segment_origin(&self, walk: &Walk) -> Option<(f32, f32)> {
        self.at_baseline_inside(walk).then_some((0.0, 0.0))
    }

    fn item_context(&self, item: &DisplayItem, walk: &Walk) -> Option<span::Context> {
        (patch_start(item) && self.text.is_balanced() && walk.pulses.is_empty()).then(|| {
            span::Context {
                affine: walk.at().0,
                clip: walk.clips.last().copied(),
                blurred: walk.open.contains(&Scope::Blur),
            }
        })
    }

    fn note_item(&self, item: &DisplayItem, walk: &mut Walk) {
        if walk.recordings.is_empty() {
            return;
        }
        let start = walk.out.len() as u32;
        let context = self.item_context(item, walk);
        let kind = scope_kind(item);
        let inflated = walk.recordings.iter().any(|recording| !recording.segment);
        for recording in walk.recordings.iter_mut() {
            let index = recording.starts.len() as u32;
            recording.starts.push(start);
            recording
                .contexts
                .push(match recording.segment && inflated {
                    true => None,
                    false => context,
                });
            recording.matching.push(span::NONE);
            match kind {
                ScopeKind::Leaf => {}
                ScopeKind::Open(kind) => recording.pending_scopes.push((index, kind)),
                ScopeKind::Close(kind) => match recording.pending_scopes.pop() {
                    Some((open, opened)) if opened == kind => {
                        recording.matching[open as usize] = index;
                        recording.matching[index as usize] = open;
                    }
                    _ => recording.broken = true,
                },
            }
        }
    }

    fn patch_units(
        &self,
        scene: &Scene,
        id: aqua_core::NodeId,
        position: usize,
        dirty: &[usize],
    ) -> Option<Vec<(usize, usize)>> {
        let entry = self.spans.get(id)?;
        let mut units: Vec<(usize, usize)> = Vec::new();
        for at in dirty {
            let relative = at - position;
            let (_, item) = scene.item_at(*at)?;
            let partner = *entry.matching.get(relative)?;
            let unit = match (scope_kind(item), partner) {
                (ScopeKind::Leaf, _) => (relative, relative),
                (ScopeKind::Open(_), partner) if partner != span::NONE => {
                    (relative, partner as usize)
                }
                (ScopeKind::Close(_), partner) if partner != span::NONE => {
                    (partner as usize, relative)
                }
                _ => return None,
            };
            units.push(unit);
        }
        units.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for unit in units {
            match merged.last() {
                Some(last) if unit.1 <= last.1 => {}
                Some(last) if unit.0 <= last.1 => return None,
                _ => merged.push(unit),
            }
        }
        for (first, last) in &merged {
            entry.contexts.get(*first).copied().flatten()?;
            for relative in *first..=*last {
                let (_, item) = scene.item_at(position + relative)?;
                patchable(item).then_some(())?;
            }
        }
        Some(merged)
    }

    fn fresh_units(
        &mut self,
        scene: &Scene,
        id: aqua_core::NodeId,
        position: usize,
        dirty: &[usize],
        shift: (f32, f32),
        walk: &Walk,
    ) -> Option<Vec<span::Unit>> {
        let units = self.patch_units(scene, id, position, dirty)?;
        let contexts: Vec<span::Context> = {
            let entry = self.spans.get(id)?;
            units
                .iter()
                .map(|(first, _)| entry.contexts[*first])
                .collect::<Option<Vec<_>>>()?
        };
        let mut fresh = Vec::with_capacity(units.len());
        for ((first, last), context) in units.into_iter().zip(contexts) {
            let moved = span::Context {
                affine: Transform2D {
                    tx: context.affine.tx + shift.0,
                    ty: context.affine.ty + shift.1,
                    ..context.affine
                },
                clip: context.clip.map(|clip| clip.translate(shift.0, shift.1)),
                blurred: context.blurred,
            };
            let mut temporary = Walk::seeded(walk, &moved);
            let (depth, clips, open) = (
                temporary.transforms.len(),
                temporary.clips.len(),
                temporary.open.len(),
            );
            for relative in first..=last {
                let (_, item) = scene.item_at(position + relative)?;
                self.emit(item, &mut temporary);
            }
            let clean = temporary.report.pending == 0
                && temporary.report.approximated == 0
                && temporary.report.unsupported() == 0
                && !temporary.report.animated()
                && temporary.transforms.len() == depth
                && temporary.clips.len() == clips
                && temporary.open.len() == open;
            if !clean {
                return None;
            }
            fresh.push(span::Unit {
                first,
                last,
                commands: temporary.out,
            });
        }
        (self.span_stamp() == self.spans.get(id)?.stamp).then_some(fresh)
    }

    fn replay_span(
        &mut self,
        scene: &Scene,
        position: usize,
        id: aqua_core::NodeId,
        local: Option<&Transform2D>,
        walk: &mut Walk,
    ) -> Option<usize> {
        if self.spans.is_empty() {
            return None;
        }
        let origin = match local {
            Some(local) => self.span_origin(walk, local)?,
            None => self.segment_origin(walk)?,
        };
        let Some(entry) = self.spans.get(id) else {
            self.span_misses[0] += 1;
            return None;
        };
        if !touch_curves(&entry.commands, &mut self.slug_store, &mut self.path_store) {
            return None;
        }
        let (dx, dy) = (origin.0 - entry.origin.0, origin.1 - entry.origin.1);
        let whole = (dx - dx.round()).abs() < 1.0e-2 && (dy - dy.round()).abs() < 1.0e-2;
        let (dx, dy) = (dx.round(), dy.round());
        let close = position + entry.len;
        let intact = scene
            .indices(position, close + 1)
            .is_some_and(|ids| ids == entry.ids.as_slice());
        let valid = whole
            && intact
            && close < scene.len()
            && entry.environment == Self::span_environment(walk)
            && entry.stamp == self.span_stamp()
            && dx.abs() <= span::SLACK
            && dy.abs() <= span::SLACK;
        if !valid {
            let env = entry.environment == Self::span_environment(walk);
            let flags = [
                !whole,
                !intact,
                !env,
                entry.stamp != self.span_stamp(),
                dx.abs() > span::SLACK || dy.abs() > span::SLACK,
            ];
            for (i, f) in flags.iter().enumerate() {
                if *f {
                    self.span_misses[1 + i] += 1;
                }
            }
            if crate::profile::enabled() {
                self.span_trace.push(format!("invalid@{position}+{} segment={} flags={flags:?} d=({dx},{dy}) raw=({},{}) org=({},{}) local={:?} at=({},{})", entry.len, local.is_none(), origin.0 - entry.origin.0, origin.1 - entry.origin.1, origin.0, origin.1, local.map(|l| (l.tx, l.ty)), walk.at().0.tx, walk.at().0.ty));
            }
            self.spans.remove(id);
            return None;
        }
        let mut dirty: Vec<usize> = Vec::new();
        let revision = entry.revision;
        let first = position + usize::from(local.is_some());
        let reaches = scene.changed_since(revision, |at| {
            if at >= first && at <= close {
                dirty.push(at);
            }
        });
        if !reaches {
            self.span_misses[6] += 1;
            return None;
        }
        dirty.sort_unstable();
        dirty.dedup();
        let units = match dirty.is_empty() {
            true => Vec::new(),
            false => match self.fresh_units(scene, id, position, &dirty, (dx, dy), walk) {
                Some(units) => units,
                None => {
                    self.span_misses[7] += 1;
                    self.volatile_spans
                        .insert(id, (self.span_frame, close - position));
                    if crate::profile::enabled() {
                        let kinds: Vec<&'static str> = dirty
                            .iter()
                            .take(4)
                            .filter_map(|at| scene.item_at(*at).map(|(_, item)| kind_name(item)))
                            .collect();
                        self.span_trace.push(format!(
                            "patchfail@{position}+{} dirty={} kinds={kinds:?}",
                            close - position,
                            dirty.len()
                        ));
                    }
                    return None;
                }
            },
        };
        let entry = self.spans.get(id)?;
        let current = {
            let guard = self.runs.lock();
            entry.laid.iter().all(|(run, laid)| {
                guard
                    .resolve(RunId(*run))
                    .and_then(|handle| handle.layout.laid_out_shared())
                    .is_some_and(|now| Arc::ptr_eq(&now, laid))
            })
        };
        if !current {
            self.span_misses[8] += 1;
            return None;
        }
        let region = walk.clips.last().copied();
        let dropped = span::replay_around(&mut walk.out, entry, &units, dx, dy, region);
        walk.report.items += (entry.len + 1) as u32;
        walk.report.glyphs += entry.tally.glyphs;
        walk.report.curves += entry.tally.curves;
        walk.report.culled += entry.tally.culled + dropped;
        for page in &entry.pages {
            self.residency.touch_page(*page);
        }
        self.path_keys_now.extend(entry.paths.iter().copied());
        if !walk.recordings.is_empty() {
            walk.laid.extend(entry.laid.iter().cloned());
        }
        self.span_replays += 1;
        Some(close)
    }

    fn begin_span(
        &mut self,
        position: usize,
        id: aqua_core::NodeId,
        local: Option<&Transform2D>,
        walk: &mut Walk,
    ) -> bool {
        if self
            .volatile_spans
            .get(&id)
            .is_some_and(|(seen, _)| self.span_frame - seen < VOLATILE_FRAMES)
        {
            return false;
        }
        let origin = match local {
            Some(local) => self.span_origin(walk, local),
            None => self.segment_origin(walk),
        };
        let Some(origin) = origin else {
            return false;
        };
        let unsupported = walk.report.unsupported();
        walk.recordings.push(Recording {
            id,
            segment: local.is_none(),
            open: position,
            depth: walk.transforms.len(),
            starts: Vec::new(),
            contexts: Vec::new(),
            matching: Vec::new(),
            pending_scopes: Vec::new(),
            broken: false,
            tainted: false,
            laid_start: walk.laid.len(),
            out_start: walk.out.len(),
            paths_start: self.path_keys_now.len(),
            marks: Marks {
                pending: walk.report.pending,
                approximated: walk.report.approximated,
                unsupported,
                glyphs: walk.report.glyphs,
                curves: walk.report.curves,
                pulsing: walk.report.animated(),
            },
            environment: Self::span_environment(walk),
            origin,
            clip: walk.clips.last().copied(),
            stamp: self.span_stamp(),
        });
        if local.is_some() {
            if let Some(top) = walk.clips.last_mut() {
                *top = top.inflate(span::SLACK);
            }
        }
        true
    }

    fn finish_span(&mut self, scene: &Scene, close: usize, walk: &mut Walk) {
        let Some(recording) = walk.recordings.pop() else {
            return;
        };
        if let (false, Some(top), Some(original)) =
            (recording.segment, walk.clips.last_mut(), recording.clip)
        {
            *top = original;
        }
        let laid = walk
            .laid
            .split_off(recording.laid_start.min(walk.laid.len()));
        let clean = !recording.tainted
            && walk.open.len() == recording.environment.open
            && self.text.is_balanced()
            && walk.report.pending == recording.marks.pending
            && walk.report.approximated == recording.marks.approximated
            && walk.report.unsupported() == recording.marks.unsupported
            && walk.report.animated() == recording.marks.pulsing
            && self.span_stamp() == recording.stamp
            && span::cacheable(&walk.out[recording.out_start..]) == span::Taint::Clean
            && walk.out.len() - recording.out_start >= span::MIN_COMMANDS;
        if !clean && crate::profile::enabled() {
            let taint = span::cacheable(&walk.out[recording.out_start..]);
            self.span_trace.push(format!(
                "unclean@{}+{} cmds={} tainted={} open={}/{} balanced={} pend={} approx={} unsup={} pulse={} stamp={} cache={taint:?}",
                recording.open,
                close - recording.open,
                walk.out.len() - recording.out_start,
                recording.tainted,
                walk.open.len(),
                recording.environment.open,
                self.text.is_balanced(),
                walk.report.pending != recording.marks.pending,
                walk.report.approximated != recording.marks.approximated,
                walk.report.unsupported() != recording.marks.unsupported,
                walk.report.animated() != recording.marks.pulsing,
                self.span_stamp() != recording.stamp,
            ));
        }
        if !walk.recordings.is_empty() {
            walk.laid.extend(laid.iter().cloned());
        }
        let full = walk.out.split_off(recording.out_start);
        let mut starts: Vec<u32> = recording
            .starts
            .iter()
            .map(|start| start - recording.out_start as u32)
            .collect();
        starts.push(full.len() as u32);
        let aligned = starts.len() == close - recording.open + 2
            && recording.contexts.len() == close - recording.open + 1;
        let aligned = aligned && !recording.broken && recording.pending_scopes.is_empty();
        let contexts = match aligned {
            true => recording.contexts,
            false => vec![None; close - recording.open + 1],
        };
        let matching = match aligned {
            true => recording.matching,
            false => vec![span::NONE; close - recording.open + 1],
        };
        if !aligned {
            starts = (0..close - recording.open + 2)
                .map(|_| full.len() as u32)
                .collect();
        }
        let region = walk.clips.last().copied();
        span::replay(&mut walk.out, &full, 0.0, 0.0, region);
        if !clean {
            self.spans.remove(recording.id);
            return;
        }
        let pages = span::pages_of(&full);
        let bytes = span::bytes_of(&full);
        let entry = span::Entry {
            starts,
            contexts,
            matching,
            commands: full,
            segment: recording.segment,
            len: close - recording.open,
            ids: scene
                .indices(recording.open, close + 1)
                .map(<[u32]>::to_vec)
                .unwrap_or_default(),
            revision: scene.revision(),
            origin: recording.origin,
            environment: recording.environment,
            stamp: recording.stamp,
            tally: span::Tally {
                glyphs: walk.report.glyphs - recording.marks.glyphs,
                curves: walk.report.curves - recording.marks.curves,
                culled: 0,
            },
            pages,
            laid,
            paths: self.path_keys_now[recording.paths_start..].to_vec(),
            bytes,
        };
        self.spans.insert(recording.id, entry);
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
            DisplayItem::SrgbaRect { bounds, fill } => {
                walk.emit_box(
                    at.rect(*bounds),
                    Radii::uniform(0.0),
                    Some(Paint::Solid(fill.to_oklch())),
                    None,
                );
            }
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
                let rect = outward(at.rect(*bounds));
                walk.open_clip(rect);
                walk.out.push(PaintCommand::PushClip(rect));
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
                let next = snapped_nest(walk, at, local);
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
            DisplayItem::PopTransform => match walk.transforms.pop() {
                None => walk.skip(Unsupported::UnbalancedScope),
                Some(_) => {
                    walk.closed = walk.recordings.last().is_some_and(|recording| {
                        !recording.segment && recording.depth == walk.transforms.len()
                    });
                }
            },
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
            DisplayItem::Shape {
                bounds,
                program,
                target,
                transition,
                ..
            } => {
                if walk.hidden(at.rect(*bounds), 0.0) {
                    return;
                }
                let (commands, animated) = self.shapes.paint(
                    walk.item,
                    *bounds,
                    program,
                    *target,
                    *transition,
                    walk.now_secs,
                );
                walk.report.shape_animation |= animated;
                for command in commands {
                    if !matches!(command, DisplayItem::Shape { .. }) {
                        self.emit(&command, walk);
                    }
                }
            }
            DisplayItem::Shader {
                bounds,
                program,
                parameters,
                animated,
            } => {
                let destination = snapped(at.rect(*bounds));
                if destination.w <= 0.0 || destination.h <= 0.0 || walk.hidden(destination, 0.0) {
                    return;
                }
                let time = match animated {
                    true => self.shader_time.map_or(walk.now_secs, Instant::as_secs_f64) as f32,
                    false => 0.0,
                };
                let mut params = vec![destination.w, destination.h, time, s];
                params.extend_from_slice(parameters);
                let spec = aqua_graph::CustomPassSpec {
                    isolate_backdrop: false,
                    name: program.name.to_string(),
                    hlsl: program.hlsl.to_string(),
                    entry: program.entry.to_string(),
                    bounds: destination,
                    params,
                    reference: program.reference,
                };
                walk.out.push(PaintCommand::PushCustomEffect {
                    bounds: destination,
                    spec,
                });
                walk.out.push(PaintCommand::PopCustomEffect);
                walk.report.shader_animation |= *animated && self.shader_time.is_none();
            }
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
        walk.open_clip(rect);
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
            guard.resolve(RunId(run_id)).and_then(|handle| {
                handle
                    .layout
                    .laid_out_shared()
                    .map(|laid| (laid, handle.layout.scale()))
            })
        };
        let Some((laid, run_scale)) = shared else {
            walk.report.pending += 1;
            return;
        };
        if !walk.recordings.is_empty() {
            walk.laid.push((run_id, laid.clone()));
        }
        let at = walk.at();
        let scene_scale = at.scale();
        let s = scene_scale / run_scale;
        let (base_x, base_y) = at.point(origin[0], origin[1]);
        if walk.cull.is_some() && at.is_exact() {
            let pad = TEXT_OVERHANG_PAD * scene_scale + TEXT_OVERHANG_RATIO * laid.size.1 * s;
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
            && (!at.is_exact() || (scene_scale / walk.base.scale() - 1.0).abs() > 1.0e-3);
        let mut curves: Vec<SlugInstance> = Vec::new();
        let signature = self
            .text
            .cache_signature()
            .filter(|_| !moving && phase == 0.0)
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
                    u32::from(self.outline_text),
                ]
            });
        let stamp = self.span_stamp();
        if let Some(signature) = signature {
            if let Some(entry) = self.run_cache.entries.get(&run_id).filter(|entry| {
                entry.signature == signature
                    && entry.stamp == stamp
                    && (entry.curves == 0 || entry.base == (base_x, base_y))
                    && Arc::ptr_eq(&entry.laid, &laid)
                    && touch_curves(&entry.commands, &mut self.slug_store, &mut self.path_store)
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
                walk.report.curves += entry.curves;
                walk.report.cached_text_runs += 1;
                for page in &entry.pages {
                    self.residency.touch_page(*page);
                }
                return;
            }
        }
        let started = walk.out.len();
        let glyphs_before = walk.report.glyphs;
        let curves_before = walk.report.curves;
        let skipped_before = walk.report.unsupported();
        let mut pages: Vec<PageId> = Vec::new();
        let fonts_revision = self.fonts.revision();
        for line in &laid.lines {
            for glyph in &line.glyphs {
                if moving || self.outline_text || glyph.font_size_px * s >= SLUG_TEXT_PX {
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
                                (plain, phase, map_bounds, scene_scale, center),
                                &mut last_color,
                            );
                            let pen = at.point(
                                origin[0] + (glyph.x + glyph.x_offset) / run_scale,
                                origin[1]
                                    + (line.baseline_y + glyph.y - glyph.y_offset) / run_scale,
                            );
                            let size = glyph.font_size_px / run_scale;
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
                            (plain, phase, map_bounds, scene_scale, center),
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
                coverage: Coverage::Text,
                glyphs: glyphs.into(),
            }));
            run_pens.push(pens);
        }
        if !curves.is_empty() {
            walk.out.push(PaintCommand::Slug(SlugRun {
                store: STORE_HANDLE,
                glyphs: curves.into(),
            }));
        }
        walk.out.extend(color_images);
        if let Some(signature) = signature
            .filter(|_| walk.report.unsupported() == skipped_before && self.span_stamp() == stamp)
        {
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
                    stamp,
                    base: (base_x, base_y),
                    bytes: run_bytes(&commands),
                    commands,
                    pens,
                    pages,
                    glyphs: walk.report.glyphs - glyphs_before,
                    curves: walk.report.curves - curves_before,
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
        if let Some((key, scene)) = self.vectors.curves(&source) {
            if self.emit_curve_scene(key, &scene, physical, opacity, walk) {
                return;
            }
        }
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
        let described = std::time::Instant::now();
        let Some(raster) = self.vectors.path(path, at.0) else {
            walk.skip(Unsupported::PathFailed);
            return;
        };
        let described = described.elapsed();
        if self.emit_curve_path(&raster, walk) {
            return;
        }
        let tint = match path.background {
            GpuiBackground::Solid(value) => Some(color::oklch(color::hsla(value)).with_alpha(1.0)),
            GpuiBackground::LinearGradient { stops, .. } => {
                let first = color::hsla(stops[0].color);
                let second = color::hsla(stops[1].color);
                (first[..3] == second[..3]).then(|| color::oklch(first).with_alpha(1.0))
            }
        };
        let mode = match tint {
            Some(_) => VectorMode::Mask,
            None => VectorMode::Color,
        };
        for raster in self.vectors.tiles(raster) {
            let resident = std::time::Instant::now();
            let slot = self.residency.vector(
                &raster.source,
                raster.width,
                raster.height,
                mode,
            );
            crate::profile::note(|| {
                format!(
                    "path_raster={}x{} describe_us={} resident_us={} style={}",
                    raster.width,
                    raster.height,
                    described.as_micros(),
                    resident.elapsed().as_micros(),
                    path_style_label(path)
                )
            });
            match slot {
                Ok(Slot::Resident(placement)) => {
                    self.path_keys_now.push(Residency::vector_key(
                        &raster.source,
                        raster.width,
                        raster.height,
                    ));
                    match tint {
                        Some(color) => walk.out.push(PaintCommand::Glyphs(GlyphRun {
                            atlas: texture_handle(placement.page),
                            coverage: Coverage::Exact,
                            glyphs: vec![Glyph {
                                bounds: raster.placement,
                                texels: texel_rect(&placement, full_source()),
                                color,
                            }].into(),
                        })),
                        None => push_image(
                        &mut walk.out,
                        &placement,
                        raster.placement,
                        full_source(),
                        1.0,
                        0.0,
                    ),
                    }
                }
                Ok(Slot::Blank { .. }) => {}
                Err(_) => walk.skip(Unsupported::PathFailed),
            }
        }
    }

    fn emit_curve_scene(
        &mut self,
        key: u64,
        scene: &aqua_media::vector::CurveScene,
        physical: Rect,
        opacity: f32,
        walk: &mut Walk,
    ) -> bool {
        use aqua_media::vector::CurveOp;
        let mut stored = Vec::new();
        for (index, op) in scene.ops.iter().enumerate() {
            if let CurveOp::Paint(glyph) = op {
                let key = key ^ (index as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15);
                let entry = match self.path_store.touch(&key) {
                    Some(entry) => entry,
                    None => self.path_store.insert_bounded(key, glyph),
                };
                let StoreLookup::Stored(entry) = entry else {
                    return false;
                };
                stored.push(entry);
            }
        }
        let bounds = scene.view_box;
        let mut scale = [physical.w / bounds.w, physical.h / bounds.h];
        if scene.contain {
            scale = [scale[0].min(scale[1]); 2];
        }
        let origin = [
            physical.x + (physical.w - bounds.w * scale[0]) * 0.5 - bounds.x * scale[0],
            physical.y + (physical.h - bounds.h * scale[1]) * 0.5 - bounds.y * scale[1],
        ];
        let linear = Linear2::scale(scene.unit * scale[0], scene.unit * scale[1]);
        walk.out.push(PaintCommand::PushClip(physical));
        walk.out.push(PaintCommand::PushOpacity(opacity));
        let mut glyphs = stored.into_iter();
        for op in &scene.ops {
            match op {
                CurveOp::PushOpacity(value) => walk.out.push(PaintCommand::PushOpacity(*value)),
                CurveOp::PopOpacity => walk.out.push(PaintCommand::PopOpacity),
                CurveOp::Paint(_) => {
                    let Some(entry) = glyphs.next() else {
                        unreachable!()
                    };
                    walk.out.push(PaintCommand::Slug(SlugRun {
                        store: PATH_STORE_HANDLE,
                        glyphs: vec![SlugInstance {
                            origin,
                            linear,
                            glyph: entry.index,
                            em_bounds: entry.em_bounds,
                            color: Oklch::WHITE,
                            clip: None,
                            opacity: 1.0,
                        }]
                        .into(),
                    }));
                    walk.report.curves += 1;
                }
            }
        }
        walk.out.push(PaintCommand::PopOpacity);
        walk.out.push(PaintCommand::PopClip);
        true
    }

    fn emit_curve_path(&mut self, raster: &super::vector::PathRaster, walk: &mut Walk) -> bool {
        use aqua_core::vector::VectorSource;
        let VectorSource::Drawing(drawing) = &raster.source else {
            return false;
        };
        let [path] = drawing.paths.as_slice() else {
            return false;
        };
        let unit = raster.width.max(raster.height) as f32;
        let stored = match self.path_store.touch(&raster.geometry_key) {
            Some(stored) => stored,
            None => match aqua_media::vector::curve_outline(path, unit) {
                Ok(Some(glyph)) => self.path_store.insert_bounded(raster.geometry_key, &glyph),
                Ok(None) | Err(_) => return false,
            },
        };
        match stored {
            StoreLookup::Stored(stored) => {
                walk.out.push(PaintCommand::Slug(SlugRun {
                    store: PATH_STORE_HANDLE,
                    glyphs: vec![SlugInstance {
                        origin: [raster.placement.x, raster.placement.y],
                        linear: Linear2::scale(unit, unit),
                        glyph: stored.index,
                        em_bounds: stored.em_bounds,
                        color: Oklch::WHITE,
                        clip: None,
                        opacity: 1.0,
                    }]
                    .into(),
                }));
                walk.report.curves += 1;
                true
            }
            StoreLookup::Blank => true,
            StoreLookup::Unsupported => false,
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
            self.spans.clear();
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

    fn retire_reshaped_paths(&mut self) {
        let now = std::mem::take(&mut self.path_keys_now);
        let previous = std::mem::replace(&mut self.path_keys, now);
        if previous.len() != self.path_keys.len() {
            return;
        }
        previous
            .iter()
            .zip(self.path_keys.iter())
            .filter(|(old, new)| old != new && !self.path_keys.contains(old))
            .for_each(|(old, _)| {
                let previous_epoch = self.residency.eviction_epoch();
                self.residency.remove_vector(*old, VectorMode::Color);
                self.residency.remove_vector(*old, VectorMode::Mask);
                self.spans
                    .retire_path(*old, previous_epoch, self.residency.eviction_epoch());
            });
    }

    pub fn begin_transition(&mut self, settings: Settings) {
        self.begun = Some(settings);
    }

    pub fn set_text_raster(&mut self, mode: TextRasterMode) {
        self.spans.clear();
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

fn is_translation(local: &Transform2D) -> bool {
    local.a == 1.0 && local.d == 1.0 && local.b == 0.0 && local.c == 0.0
}

fn snapped_nest(walk: &Walk, at: Affine, local: &Transform2D) -> Affine {
    let next = at.nested(*local);
    let translated =
        at.0.b == 0.0 && at.0.c == 0.0 && at.0.a == walk.base.0.a && at.0.d == walk.base.0.d;
    match translated && !walk.lifted && is_translation(local) && next.finite() {
        true => Affine(Transform2D {
            tx: (next.0.tx + 0.5).floor(),
            ty: (next.0.ty + 0.5).floor(),
            ..next.0
        }),
        false => next,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nesting {
    Transform,
    Clip,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeKind {
    Leaf,
    Open(Nesting),
    Close(Nesting),
}

fn scope_kind(item: &DisplayItem) -> ScopeKind {
    match item {
        DisplayItem::PushTransform(_) => ScopeKind::Open(Nesting::Transform),
        DisplayItem::PopTransform => ScopeKind::Close(Nesting::Transform),
        DisplayItem::PushClip { .. }
        | DisplayItem::PushRoundedClip { .. }
        | DisplayItem::PushGpuiContentMask { .. } => ScopeKind::Open(Nesting::Clip),
        DisplayItem::PopClip => ScopeKind::Close(Nesting::Clip),
        DisplayItem::PushOpacity(_)
        | DisplayItem::PushContentBlur { .. }
        | DisplayItem::PushTextColorMap(_)
        | DisplayItem::PushGpuiTextColorMap(_)
        | DisplayItem::PushGpuiTextShimmer(_)
        | DisplayItem::PushGpuiTextColor(_)
        | DisplayItem::PushGpuiTextOpacity(_)
        | DisplayItem::PushGpuiSourceTextColor(_)
        | DisplayItem::PushGpuiQuadPulse(_) => ScopeKind::Open(Nesting::Other),
        DisplayItem::PopOpacity
        | DisplayItem::PopContentBlur
        | DisplayItem::PopTextColorMap
        | DisplayItem::PopGpuiTextColorMap
        | DisplayItem::PopGpuiTextColor
        | DisplayItem::PopGpuiTextOpacity
        | DisplayItem::PopGpuiSourceTextColor
        | DisplayItem::PopGpuiQuadPulse => ScopeKind::Close(Nesting::Other),
        _ => ScopeKind::Leaf,
    }
}

fn patchable(item: &DisplayItem) -> bool {
    match scope_kind(item) {
        ScopeKind::Open(Nesting::Other) | ScopeKind::Close(Nesting::Other) => false,
        ScopeKind::Open(_) | ScopeKind::Close(_) => true,
        ScopeKind::Leaf => matches!(
            item,
            DisplayItem::Rect { .. }
                | DisplayItem::Quad { .. }
                | DisplayItem::GpuiBackground { .. }
                | DisplayItem::GpuiBorder { .. }
                | DisplayItem::Shadow { .. }
                | DisplayItem::GpuiShadow { .. }
                | DisplayItem::GpuiSvg { .. }
                | DisplayItem::GpuiTransformedSvg { .. }
        ),
    }
}

fn patch_start(item: &DisplayItem) -> bool {
    patchable(item) && !matches!(scope_kind(item), ScopeKind::Close(_))
}

fn taints_span(item: &DisplayItem) -> bool {
    matches!(
        item,
        DisplayItem::PushGpuiTextShimmer(_)
            | DisplayItem::PushGpuiQuadPulse(_)
            | DisplayItem::External { .. }
            | DisplayItem::Shader { .. }
            | DisplayItem::Shape { .. }
    )
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
    fn retained_solids(&self) -> bool {
        !self.transition_active()
    }

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

    fn release_uploads(&mut self) -> UploadBatch {
        let mut uploads = aqua_media::upload::upload_batch(self.residency.release_updates());
        uploads.textures.extend(self.slug_store.uploads().textures);
        let mut paths = self.path_store.uploads();
        for upload in &mut paths.textures {
            upload.handle = PATH_STORE_HANDLE;
        }
        uploads.textures.extend(paths.textures);
        uploads
    }

    fn trim(&mut self, request: TrimRequest) -> usize {
        match request {
            TrimRequest::IdleFrames(frames) => {
                self.spans.clear();
                self.residency.trim_idle(frames)
            }
            TrimRequest::Everything => {
                let curves = self.slug_store.len() + self.path_store.len();
                self.spans.clear();
                self.run_cache.clear();
                self.vectors.clear();
                if self.transition.is_none() {
                    self.last_commands.clear();
                }
                self.slug_generation += 1;
                match self.transition.is_none() {
                    true => {
                        self.slug_store.release_resources();
                        self.path_store.release_resources();
                    }
                    false => {
                        self.slug_store.clear();
                        self.path_store.clear();
                    }
                }
                self.slug_cache.clear();
                if let Some(stats) = &self.stats {
                    stats.slug_bytes.store(
                        self.slug_store.resident_bytes() + self.path_store.resident_bytes(),
                        Ordering::Relaxed,
                    );
                }
                curves + self.residency.trim_idle(0)
            }
        }
    }
}
