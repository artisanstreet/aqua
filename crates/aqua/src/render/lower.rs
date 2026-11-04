use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aqua_core::geometry::{CornerRadii, Rect};
use aqua_core::image::ImagePlacement;
use aqua_core::scene::DisplayItem;
use aqua_core::{Oklch, Point, Size, Transform2D};
use aqua_graph::{
    plan_paint_with, BackdropBlur, Extent, FramePlan, Glyph, GlyphRun, Image, PaintCommand,
    PlanError, PlanLimits, Quad, Shadow,
};
use aqua_media::atlas::{AtlasConfig, ImageKey, Placement, Slot, StreamBudget};
use aqua_media::residency::{GlyphSlot, Residency, ResidencyConfig, VectorMode};
use aqua_media::upload::texture_handle;
use aqua_protocol::UploadBatch;
use aqua_scene::Scene;
use aqua_text::raster::{quantize_size, quantize_x, raster_key};
use aqua_text::residency::GlyphResolver;
use aqua_text::runs::RunId;
use aqua_text::{SharedFonts, SharedRuns};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unsupported {
    GpuiItem,
    InsetShadow,
    GeneralTransform,
    DashedBorder,
    UnresolvedText,
    UnknownImage,
    VectorFailed,
    GlyphBudget,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LowerReport {
    pub items: u32,
    pub commands: u32,
    pub glyphs: u32,
    pub skipped: BTreeMap<Unsupported, u32>,
}

#[derive(Debug)]
pub struct Lowered {
    pub uploads: UploadBatch,
    pub plan: FramePlan,
    pub report: LowerReport,
}

#[derive(Clone)]
struct RetainedImage {
    width: u32,
    height: u32,
    premultiplied: Arc<[u8]>,
}

#[derive(Clone, Copy)]
struct Affine {
    scale: f32,
    tx: f32,
    ty: f32,
}

impl Affine {
    fn rect(self, rect: Rect) -> Rect {
        Rect {
            x: rect.x * self.scale + self.tx,
            y: rect.y * self.scale + self.ty,
            w: rect.w * self.scale,
            h: rect.h * self.scale,
        }
    }

    fn point(self, x: f32, y: f32) -> (f32, f32) {
        (x * self.scale + self.tx, y * self.scale + self.ty)
    }

    fn compose(self, local: Transform2D) -> Option<Affine> {
        let axis_aligned = local.b == 0.0 && local.c == 0.0 && (local.a - local.d).abs() < 1e-6;
        axis_aligned.then_some(Affine {
            scale: self.scale * local.a,
            tx: self.tx + local.tx * self.scale,
            ty: self.ty + local.ty * self.scale,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Clip,
    Opacity,
    Blur,
}

#[derive(Default)]
struct Scopes {
    open: Vec<Scope>,
    transforms: Vec<Affine>,
}

impl Scopes {
    fn close(&mut self, kind: Scope) -> bool {
        match self.open.last() {
            Some(top) if *top == kind => {
                self.open.pop();
                true
            }
            _ => false,
        }
    }
}

pub struct Lowering {
    fonts: SharedFonts,
    runs: SharedRuns,
    residency: Residency,
    resolver: GlyphResolver,
    retained: HashMap<u64, RetainedImage>,
    clear: Oklch,
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

fn max_radius(corners: CornerRadii) -> f32 {
    corners
        .top_left
        .max(corners.top_right)
        .max(corners.bottom_right)
        .max(corners.bottom_left)
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

impl Lowering {
    pub fn new(fonts: SharedFonts, runs: SharedRuns) -> Self {
        Self {
            fonts,
            runs,
            residency: Residency::new(residency_config()),
            resolver: GlyphResolver::new(),
            retained: HashMap::new(),
            clear: Oklch::TRANSPARENT,
        }
    }

    pub fn with_clear(mut self, clear: Oklch) -> Self {
        self.clear = clear;
        self
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
    }

    pub fn reset(&mut self) {
        self.residency = Residency::new(residency_config());
        self.resolver = GlyphResolver::new();
    }

    pub fn trim(&mut self, idle_frames: u64) -> usize {
        self.residency.trim_idle(idle_frames)
    }

    pub fn residency(&self) -> &Residency {
        &self.residency
    }

    pub fn lower(
        &mut self,
        scene: &Scene,
        size: Size,
        scale: f32,
        limits: PlanLimits,
    ) -> Result<Lowered, PlanError> {
        self.residency.begin_frame();
        let mut report = LowerReport::default();
        let mut commands: Vec<PaintCommand> = Vec::with_capacity(scene.len() * 2);
        let mut scopes = Scopes::default();
        let base = Affine {
            scale,
            tx: 0.0,
            ty: 0.0,
        };
        let extent = physical_extent(size, scale);
        for (_, item) in scene.ordered() {
            report.items += 1;
            self.emit(item, base, &mut scopes, &mut commands, &mut report);
        }
        close_scopes(&mut scopes, &mut commands);
        report.commands = commands.len() as u32;
        let updates = self.residency.finish_frame();
        let uploads = aqua_media::upload::upload_batch(updates);
        let plan = plan_paint_with(extent, self.clear, &commands, limits)?;
        Ok(Lowered {
            uploads,
            plan,
            report,
        })
    }

    fn current(scopes: &Scopes, base: Affine) -> Affine {
        scopes.transforms.last().copied().unwrap_or(base)
    }

    fn emit(
        &mut self,
        item: &DisplayItem,
        base: Affine,
        scopes: &mut Scopes,
        out: &mut Vec<PaintCommand>,
        report: &mut LowerReport,
    ) {
        let at = Self::current(scopes, base);
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
                if let Some((ox, oy, blur, color)) = shadow {
                    out.push(PaintCommand::Shadow(Shadow {
                        bounds,
                        radius: radius * at.scale,
                        offset: [ox * at.scale, oy * at.scale],
                        sigma: blur * at.scale * 0.5,
                        spread: 0.0,
                        color: *color,
                    }));
                }
                out.push(PaintCommand::Quad(Quad {
                    bounds,
                    radius: radius * at.scale,
                    border_width: border_width * at.scale,
                    fill: *fill,
                    border_color: *border_color,
                }));
            }
            DisplayItem::Quad {
                bounds,
                corners,
                fill,
                border_widths,
                border_color,
                dashed,
            } => {
                if *dashed {
                    skip(report, Unsupported::DashedBorder);
                }
                let widest = border_widths
                    .left
                    .max(border_widths.right)
                    .max(border_widths.top)
                    .max(border_widths.bottom);
                out.push(PaintCommand::Quad(Quad {
                    bounds: at.rect(*bounds),
                    radius: max_radius(*corners) * at.scale,
                    border_width: widest * at.scale,
                    fill: *fill,
                    border_color: *border_color,
                }));
            }
            DisplayItem::Shadow {
                bounds,
                corners,
                layer,
            } => {
                if layer.inset {
                    skip(report, Unsupported::InsetShadow);
                } else {
                    out.push(PaintCommand::Shadow(Shadow {
                        bounds: at.rect(*bounds),
                        radius: max_radius(*corners) * at.scale,
                        offset: [layer.dx * at.scale, layer.dy * at.scale],
                        sigma: layer.blur * at.scale * 0.5,
                        spread: layer.spread * at.scale,
                        color: layer.color,
                    }));
                }
            }
            DisplayItem::PushClip { bounds, .. } | DisplayItem::PushRoundedClip { bounds, .. } => {
                scopes.open.push(Scope::Clip);
                out.push(PaintCommand::PushClip(at.rect(*bounds)));
            }
            DisplayItem::PopClip => {
                if scopes.close(Scope::Clip) {
                    out.push(PaintCommand::PopClip);
                }
            }
            DisplayItem::PushOpacity(opacity) => {
                scopes.open.push(Scope::Opacity);
                out.push(PaintCommand::PushOpacity(opacity.clamp(0.0, 1.0)));
            }
            DisplayItem::PopOpacity => {
                if scopes.close(Scope::Opacity) {
                    out.push(PaintCommand::PopOpacity);
                }
            }
            DisplayItem::PushContentBlur { bounds, radius } => {
                scopes.open.push(Scope::Blur);
                out.push(PaintCommand::PushContentBlur {
                    bounds: at.rect(*bounds),
                    radius: radius * at.scale,
                });
            }
            DisplayItem::PopContentBlur => {
                if scopes.close(Scope::Blur) {
                    out.push(PaintCommand::PopContentBlur);
                }
            }
            DisplayItem::BackdropBlur { bounds, radius } => {
                out.push(PaintCommand::BackdropBlur(BackdropBlur {
                    bounds: at.rect(*bounds),
                    radius: radius * at.scale,
                    corner_radius: 0.0,
                }));
            }
            DisplayItem::BackdropBlurRounded {
                bounds,
                radius,
                corners,
            } => {
                out.push(PaintCommand::BackdropBlur(BackdropBlur {
                    bounds: at.rect(*bounds),
                    radius: radius * at.scale,
                    corner_radius: max_radius(*corners) * at.scale,
                }));
            }
            DisplayItem::PushTransform(local) => match at.compose(*local) {
                Some(next) => scopes.transforms.push(next),
                None => {
                    skip(report, Unsupported::GeneralTransform);
                    scopes.transforms.push(at);
                }
            },
            DisplayItem::PopTransform => {
                scopes.transforms.pop();
            }
            DisplayItem::Text {
                at: origin,
                run,
                color,
                opacity,
            } => self.emit_text(*origin, run.id, *color, *opacity, at, out, report),
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
                *rounded_clip * at.scale,
                out,
                report,
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
                *rounded_clip * at.scale,
                out,
                report,
            ),
            DisplayItem::Vector {
                bounds,
                source,
                opacity,
            } => {
                let physical = at.rect(*bounds);
                let width = physical.w.round().max(1.0) as u32;
                let height = physical.h.round().max(1.0) as u32;
                match self
                    .residency
                    .vector(source, width, height, VectorMode::Color)
                {
                    Ok(Slot::Resident(placement)) => {
                        push_image(out, &placement, physical, full_source(), *opacity, 0.0)
                    }
                    Ok(Slot::Blank { .. }) => {}
                    Err(_) => skip(report, Unsupported::VectorFailed),
                }
            }
            DisplayItem::PushTextColorMap(_)
            | DisplayItem::PopTextColorMap
            | DisplayItem::PushGpuiTextColorMap(_)
            | DisplayItem::PushGpuiTextShimmer(_)
            | DisplayItem::PopGpuiTextColorMap
            | DisplayItem::PushGpuiTextColor(_)
            | DisplayItem::PopGpuiTextColor
            | DisplayItem::PushGpuiTextOpacity(_)
            | DisplayItem::PopGpuiTextOpacity
            | DisplayItem::PushGpuiSourceTextColor(_)
            | DisplayItem::PopGpuiSourceTextColor
            | DisplayItem::PushGpuiQuadPulse(_)
            | DisplayItem::PopGpuiQuadPulse
            | DisplayItem::PushGpuiContentMask { .. }
            | DisplayItem::GpuiSvg { .. }
            | DisplayItem::GpuiTransformedSvg { .. }
            | DisplayItem::GpuiShadow { .. }
            | DisplayItem::GpuiPath { .. }
            | DisplayItem::GpuiMesh { .. }
            | DisplayItem::GpuiBorder { .. }
            | DisplayItem::GpuiBackground { .. } => skip(report, Unsupported::GpuiItem),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_text(
        &mut self,
        origin: [f32; 2],
        run_id: u64,
        color: Oklch,
        opacity: f32,
        at: Affine,
        out: &mut Vec<PaintCommand>,
        report: &mut LowerReport,
    ) {
        let runs = self.runs.clone();
        let guard = runs.lock();
        let Some(handle) = guard.resolve(RunId(run_id)) else {
            skip(report, Unsupported::UnresolvedText);
            return;
        };
        let Some(laid) = handle.layout.laid_out() else {
            skip(report, Unsupported::UnresolvedText);
            return;
        };
        let (base_x, base_y) = at.point(origin[0], origin[1]);
        let mut batches: Vec<(aqua_media::atlas::PageId, Vec<Glyph>)> = Vec::new();
        let mut color_images: Vec<PaintCommand> = Vec::new();
        for line in &laid.lines {
            for glyph in &line.glyphs {
                let x = base_x + (glyph.x + glyph.x_offset) * at.scale;
                let (pixel_x, subpixel) = quantize_x(x);
                let y = (base_y + (glyph.y - glyph.y_offset) * at.scale).trunc() as i32;
                let mut key = raster_key(glyph, subpixel);
                key.size_bits = quantize_size(glyph.font_size_px * at.scale).to_bits();
                let resolved = {
                    let mut fonts = self.fonts.lock();
                    self.resolver.resolve(&mut self.residency, &mut fonts, &key)
                };
                let slot = match resolved {
                    Ok(slot) => slot,
                    Err(_) => {
                        skip(report, Unsupported::GlyphBudget);
                        continue;
                    }
                };
                let (GlyphSlot::Mask(Slot::Resident(placement))
                | GlyphSlot::Color(Slot::Resident(placement))) = slot
                else {
                    continue;
                };
                report.glyphs += 1;
                let glyph_color = glyph
                    .color_srgb
                    .map(|[r, g, b, a]| Oklch::from_srgb8(r, g, b, f32::from(a) / 255.0))
                    .unwrap_or(color);
                let bounds = Rect {
                    x: (pixel_x + placement.bearing[0]) as f32,
                    y: ((line.baseline_y * at.scale).round() as i32 + y - placement.bearing[1])
                        as f32,
                    w: placement.width as f32,
                    h: placement.height as f32,
                };
                match slot {
                    GlyphSlot::Mask(_) => {
                        let tinted = glyph_color.with_alpha(glyph_color.alpha * opacity);
                        let entry = Glyph {
                            bounds,
                            texels: texel_rect(&placement, full_source()),
                            color: tinted,
                        };
                        match batches.last_mut() {
                            Some((page, glyphs)) if *page == placement.page => glyphs.push(entry),
                            _ => batches.push((placement.page, vec![entry])),
                        }
                    }
                    GlyphSlot::Color(_) => {
                        let mut image = Vec::new();
                        push_image(&mut image, &placement, bounds, full_source(), opacity, 0.0);
                        color_images.extend(image);
                    }
                }
            }
        }
        for (page, glyphs) in batches {
            out.push(PaintCommand::Glyphs(GlyphRun {
                atlas: texture_handle(page),
                glyphs,
            }));
        }
        out.extend(color_images);
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_image(
        &mut self,
        layer: u64,
        bounds: Rect,
        fit: Option<(aqua_core::image::ImageFit, Point, Rect)>,
        opacity: f32,
        radius: f32,
        out: &mut Vec<PaintCommand>,
        report: &mut LowerReport,
    ) {
        let Some(image) = self.retained.get(&layer).cloned() else {
            skip(report, Unsupported::UnknownImage);
            return;
        };
        let key = image_key(layer, &image);
        let slot = match self.residency.image_resident(&key) {
            Some(slot) => Ok(slot),
            None => self.residency.image(key, &image.premultiplied),
        };
        let Ok(Slot::Resident(placement)) = slot else {
            skip(report, Unsupported::UnknownImage);
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
        let overflow = clipped
            && (destination.x < bounds.x
                || destination.y < bounds.y
                || destination.right() > bounds.right()
                || destination.bottom() > bounds.bottom());
        if overflow {
            out.push(PaintCommand::PushClip(bounds));
        }
        push_image(out, &placement, destination, source, opacity, radius);
        if overflow {
            out.push(PaintCommand::PopClip);
        }
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

fn skip(report: &mut LowerReport, reason: Unsupported) {
    *report.skipped.entry(reason).or_insert(0) += 1;
}

fn close_scopes(scopes: &mut Scopes, out: &mut Vec<PaintCommand>) {
    while let Some(kind) = scopes.open.pop() {
        out.push(match kind {
            Scope::Clip => PaintCommand::PopClip,
            Scope::Opacity => PaintCommand::PopOpacity,
            Scope::Blur => PaintCommand::PopContentBlur,
        });
    }
    scopes.transforms.clear();
}
