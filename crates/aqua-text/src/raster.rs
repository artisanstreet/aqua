use skrifa::color::{Brush, ColorPainter, CompositeMode, Extend};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::types::BoundingBox;
use skrifa::{GlyphId, MetadataProvider};
use zeno::{Command, Mask, Origin, Point, Stroke, Transform, Vector};

use crate::font::{FontEntry, FontId, FontManager};
use crate::layout::LaidGlyph;
use crate::strike::{has_mask_strikes, rasterize_strike, StrikeKind};
use crate::style::FontStretch;

pub const SUBPIXEL_STEPS: u8 = 4;
pub const SIZE_STEPS: f32 = 64.0;
const ITALIC_SKEW: f32 = 0.2126;
const EMBOLDEN_EM: f32 = 1.0 / 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RasterKey {
    pub font_id: FontId,
    pub weight: u16,
    pub stretch: FontStretch,
    pub synthetic_italic: bool,
    pub glyph_id: u16,
    pub size_bits: u32,
    pub subpixel: u8,
}

impl RasterKey {
    pub fn size_px(&self) -> f32 {
        f32::from_bits(self.size_bits)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Bitmap {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlyphImage {
    Mask(Bitmap),
    Color(Bitmap),
}

impl GlyphImage {
    pub fn bitmap(&self) -> &Bitmap {
        match self {
            GlyphImage::Mask(bitmap) | GlyphImage::Color(bitmap) => bitmap,
        }
    }

    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            GlyphImage::Mask(_) => 1,
            GlyphImage::Color(_) => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphPlacement {
    pub key: RasterKey,
    pub x: i32,
    pub y: i32,
}

pub fn quantize_x(x: f32) -> (i32, u8) {
    let quarters = (x * f32::from(SUBPIXEL_STEPS)).round() as i32;
    let steps = i32::from(SUBPIXEL_STEPS);
    (quarters.div_euclid(steps), quarters.rem_euclid(steps) as u8)
}

pub fn quantize_size(size_px: f32) -> f32 {
    (size_px * SIZE_STEPS).round().max(1.0) / SIZE_STEPS
}

pub fn raster_key(glyph: &LaidGlyph, subpixel: u8) -> RasterKey {
    RasterKey {
        font_id: glyph.font_id,
        weight: glyph.font_weight,
        stretch: glyph.font_stretch,
        synthetic_italic: glyph.synthetic_italic,
        glyph_id: glyph.glyph_id,
        size_bits: quantize_size(glyph.font_size_px).to_bits(),
        subpixel,
    }
}

pub fn place_glyph(
    glyph: &LaidGlyph,
    origin_x: f32,
    baseline_y: f32,
    snap_y: bool,
) -> GlyphPlacement {
    let (x, subpixel) = quantize_x(origin_x + glyph.x + glyph.x_offset);
    let y = baseline_y + glyph.y - glyph.y_offset;
    let y = if snap_y { y.round() } else { y.floor() };
    GlyphPlacement {
        key: raster_key(glyph, subpixel),
        x,
        y: y as i32,
    }
}

struct CommandPen {
    commands: Vec<Command>,
}

impl OutlinePen for CommandPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::MoveTo(Point::new(x, y)));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::LineTo(Point::new(x, y)));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.commands
            .push(Command::QuadTo(Point::new(cx0, cy0), Point::new(x, y)));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.commands.push(Command::CurveTo(
            Point::new(cx0, cy0),
            Point::new(cx1, cy1),
            Point::new(x, y),
        ));
    }

    fn close(&mut self) {
        self.commands.push(Command::Close);
    }
}

fn outline_commands(
    font: &skrifa::FontRef<'_>,
    entry: &FontEntry,
    glyph: GlyphId,
    size: f32,
) -> Option<Vec<Command>> {
    let outline = font.outline_glyphs().get(glyph)?;
    let mut pen = CommandPen {
        commands: Vec::new(),
    };
    outline
        .draw(
            DrawSettings::unhinted(Size::new(size), LocationRef::from(entry.location())),
            &mut pen,
        )
        .ok()?;
    Some(pen.commands)
}

pub fn embolden_width(size_px: f32) -> f32 {
    size_px * EMBOLDEN_EM
}

fn mask_from_commands(
    commands: &[Command],
    subpixel: u8,
    italic: bool,
    embolden: f32,
) -> Option<Bitmap> {
    if commands.is_empty() {
        return None;
    }
    let skew = if italic { ITALIC_SKEW } else { 0.0 };
    let shift = f32::from(subpixel) / f32::from(SUBPIXEL_STEPS);
    let transform = Some(Transform::new(1.0, 0.0, skew, -1.0, shift, 0.0));
    let (data, placement) = match embolden > 0.0 {
        false => Mask::new(commands)
            .origin(Origin::TopLeft)
            .transform(transform)
            .render(),
        true => {
            let (mut data, placement) = Mask::new(commands)
                .origin(Origin::TopLeft)
                .style(Stroke::new(embolden))
                .transform(transform)
                .render();
            let mut fill = vec![0u8; data.len()];
            Mask::new(commands)
                .origin(Origin::TopLeft)
                .transform(transform)
                .size(placement.width, placement.height)
                .offset(Vector::new(
                    -(placement.left as f32),
                    -(placement.top as f32),
                ))
                .render_into(&mut fill, None);
            data.iter_mut()
                .zip(&fill)
                .for_each(|(stroke, fill)| *stroke = (*stroke).max(*fill));
            (data, placement)
        }
    };
    (placement.width > 0 && placement.height > 0).then_some(Bitmap {
        left: placement.left,
        top: -placement.top,
        width: placement.width,
        height: placement.height,
        data,
    })
}

#[derive(Default)]
pub struct Rasterizer;

impl Rasterizer {
    pub fn new() -> Self {
        Self
    }

    pub fn rasterize(&mut self, fonts: &mut FontManager, key: &RasterKey) -> Option<GlyphImage> {
        let entry = fonts.entry_with(key.font_id, key.weight, key.stretch, key.synthetic_italic)?;
        let size = key.size_px();
        entry.with_font(|font| {
            let glyph = GlyphId::new(u32::from(key.glyph_id));
            if entry.has_color() {
                if let Some(color) = rasterize_color(font, &entry, glyph, size, key.subpixel) {
                    return Some(GlyphImage::Color(color));
                }
                if let Some(image) = rasterize_strike(
                    font,
                    glyph,
                    size,
                    entry.units_per_em(),
                    key.subpixel,
                    StrikeKind::Any,
                ) {
                    return Some(image);
                }
            }
            if has_mask_strikes(font) {
                let has_outline = font.outline_glyphs().get(glyph).is_some();
                let kind = match has_outline {
                    true => StrikeKind::MasksAtExactSize,
                    false => StrikeKind::Any,
                };
                if let Some(image) =
                    rasterize_strike(font, glyph, size, entry.units_per_em(), key.subpixel, kind)
                {
                    return Some(image);
                }
            }
            let commands = outline_commands(font, &entry, glyph, size)?;
            let embolden = if entry.synthetic_bold() {
                embolden_width(size)
            } else {
                0.0
            };
            mask_from_commands(&commands, key.subpixel, key.synthetic_italic, embolden)
                .map(GlyphImage::Mask)
        })?
    }
}

#[derive(Clone)]
struct Canvas {
    pixels: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            pixels: vec![[0.0; 4]; width * height],
        }
    }
}

struct ColorRaster<'a, 'f> {
    font: &'a skrifa::FontRef<'f>,
    entry: &'a FontEntry,
    palette: Vec<[f32; 4]>,
    base: Transform,
    transforms: Vec<Transform>,
    clips: Vec<Vec<f32>>,
    layers: Vec<(Canvas, CompositeMode)>,
    width: usize,
    height: usize,
}

fn compose(local: &Transform, outer: &Transform) -> Transform {
    Transform::new(
        outer.xx * local.xx + outer.yx * local.xy,
        outer.xy * local.xx + outer.yy * local.xy,
        outer.xx * local.yx + outer.yx * local.yy,
        outer.xy * local.yx + outer.yy * local.yy,
        outer.xx * local.x + outer.yx * local.y + outer.x,
        outer.xy * local.x + outer.yy * local.y + outer.y,
    )
}

fn invert(t: &Transform) -> Option<Transform> {
    let det = t.xx * t.yy - t.xy * t.yx;
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let xx = t.yy * inv;
    let xy = -t.xy * inv;
    let yx = -t.yx * inv;
    let yy = t.xx * inv;
    Some(Transform::new(
        xx,
        xy,
        yx,
        yy,
        -(xx * t.x + yx * t.y),
        -(xy * t.x + yy * t.y),
    ))
}

impl ColorRaster<'_, '_> {
    fn current(&self) -> Transform {
        self.transforms.last().copied().unwrap_or(self.base)
    }

    fn render_mask(&self, commands: &[Command]) -> Vec<f32> {
        let mut buffer = vec![0u8; self.width * self.height];
        Mask::new(commands)
            .size(self.width as u32, self.height as u32)
            .origin(Origin::TopLeft)
            .transform(Some(self.current()))
            .render_into(&mut buffer, None);
        buffer.into_iter().map(|v| f32::from(v) / 255.0).collect()
    }

    fn brush_color(&self, palette_index: u16, alpha: f32) -> [f32; 4] {
        let color = self
            .palette
            .get(usize::from(palette_index))
            .copied()
            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
        [color[0], color[1], color[2], color[3] * alpha]
    }

    fn stop_color(&self, stops: &[skrifa::color::ColorStop], t: f32, extend: Extend) -> [f32; 4] {
        if stops.is_empty() {
            return [0.0; 4];
        }
        let t = match extend {
            Extend::Pad => t.clamp(0.0, 1.0),
            Extend::Repeat => t.rem_euclid(1.0),
            Extend::Reflect => {
                let m = t.rem_euclid(2.0);
                if m > 1.0 {
                    2.0 - m
                } else {
                    m
                }
            }
            _ => t.clamp(0.0, 1.0),
        };
        let first = &stops[0];
        if t <= first.offset {
            return self.brush_color(first.palette_index, first.alpha);
        }
        for pair in stops.windows(2) {
            if t <= pair[1].offset {
                let span = (pair[1].offset - pair[0].offset).max(1e-6);
                let f = (t - pair[0].offset) / span;
                let a = self.brush_color(pair[0].palette_index, pair[0].alpha);
                let b = self.brush_color(pair[1].palette_index, pair[1].alpha);
                return [
                    a[0] + (b[0] - a[0]) * f,
                    a[1] + (b[1] - a[1]) * f,
                    a[2] + (b[2] - a[2]) * f,
                    a[3] + (b[3] - a[3]) * f,
                ];
            }
        }
        let last = &stops[stops.len() - 1];
        self.brush_color(last.palette_index, last.alpha)
    }

    fn canvas(&mut self) -> &mut Canvas {
        &mut self.layers.last_mut().expect("base layer").0
    }
}

fn luminosity(color: [f32; 3]) -> f32 {
    0.3 * color[0] + 0.59 * color[1] + 0.11 * color[2]
}

fn clip_color(color: [f32; 3]) -> [f32; 3] {
    let lum = luminosity(color);
    let low = color[0].min(color[1]).min(color[2]);
    let high = color[0].max(color[1]).max(color[2]);
    let mut out = color;
    if low < 0.0 {
        out = out.map(|c| lum + (c - lum) * lum / (lum - low));
    }
    if high > 1.0 {
        out = out.map(|c| lum + (c - lum) * (1.0 - lum) / (high - lum));
    }
    out
}

fn set_luminosity(color: [f32; 3], lum: f32) -> [f32; 3] {
    let delta = lum - luminosity(color);
    clip_color(color.map(|c| c + delta))
}

fn saturation(color: [f32; 3]) -> f32 {
    color[0].max(color[1]).max(color[2]) - color[0].min(color[1]).min(color[2])
}

fn set_saturation(color: [f32; 3], sat: f32) -> [f32; 3] {
    let low = color[0].min(color[1]).min(color[2]);
    let high = color[0].max(color[1]).max(color[2]);
    if high <= low {
        return [0.0; 3];
    }
    color.map(|c| (c - low) * sat / (high - low))
}

fn separable(mode: CompositeMode, backdrop: f32, source: f32) -> f32 {
    match mode {
        CompositeMode::Multiply => backdrop * source,
        CompositeMode::Screen => backdrop + source - backdrop * source,
        CompositeMode::Overlay => separable(CompositeMode::HardLight, source, backdrop),
        CompositeMode::Darken => backdrop.min(source),
        CompositeMode::Lighten => backdrop.max(source),
        CompositeMode::ColorDodge => {
            if backdrop == 0.0 {
                0.0
            } else if source >= 1.0 {
                1.0
            } else {
                (backdrop / (1.0 - source)).min(1.0)
            }
        }
        CompositeMode::ColorBurn => {
            if backdrop >= 1.0 {
                1.0
            } else if source <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - backdrop) / source).min(1.0)
            }
        }
        CompositeMode::HardLight => {
            if source <= 0.5 {
                backdrop * 2.0 * source
            } else {
                let s = 2.0 * source - 1.0;
                backdrop + s - backdrop * s
            }
        }
        CompositeMode::SoftLight => {
            if source <= 0.5 {
                backdrop - (1.0 - 2.0 * source) * backdrop * (1.0 - backdrop)
            } else {
                let d = if backdrop <= 0.25 {
                    ((16.0 * backdrop - 12.0) * backdrop + 4.0) * backdrop
                } else {
                    backdrop.sqrt()
                };
                backdrop + (2.0 * source - 1.0) * (d - backdrop)
            }
        }
        CompositeMode::Difference => (backdrop - source).abs(),
        CompositeMode::Exclusion => backdrop + source - 2.0 * backdrop * source,
        _ => source,
    }
}

fn blend_colors(mode: CompositeMode, backdrop: [f32; 3], source: [f32; 3]) -> [f32; 3] {
    match mode {
        CompositeMode::HslHue => set_luminosity(
            set_saturation(source, saturation(backdrop)),
            luminosity(backdrop),
        ),
        CompositeMode::HslSaturation => set_luminosity(
            set_saturation(backdrop, saturation(source)),
            luminosity(backdrop),
        ),
        CompositeMode::HslColor => set_luminosity(source, luminosity(backdrop)),
        CompositeMode::HslLuminosity => set_luminosity(backdrop, luminosity(source)),
        _ => std::array::from_fn(|channel| separable(mode, backdrop[channel], source[channel])),
    }
}

fn is_blend_mode(mode: CompositeMode) -> bool {
    matches!(
        mode,
        CompositeMode::Screen
            | CompositeMode::Overlay
            | CompositeMode::Darken
            | CompositeMode::Lighten
            | CompositeMode::ColorDodge
            | CompositeMode::ColorBurn
            | CompositeMode::HardLight
            | CompositeMode::SoftLight
            | CompositeMode::Difference
            | CompositeMode::Exclusion
            | CompositeMode::Multiply
            | CompositeMode::HslHue
            | CompositeMode::HslSaturation
            | CompositeMode::HslColor
            | CompositeMode::HslLuminosity
    )
}

fn radial_parameter(
    point: (f32, f32),
    c0: (f32, f32),
    r0: f32,
    c1: (f32, f32),
    r1: f32,
) -> Option<f32> {
    let (cdx, cdy) = (c1.0 - c0.0, c1.1 - c0.1);
    let dr = r1 - r0;
    let (px, py) = (point.0 - c0.0, point.1 - c0.1);
    let a = cdx * cdx + cdy * cdy - dr * dr;
    let b = px * cdx + py * cdy + r0 * dr;
    let c = px * px + py * py - r0 * r0;
    let valid = |t: f32| r0 + t * dr >= 0.0;
    if a.abs() < 1e-9 {
        if b.abs() < 1e-9 {
            return None;
        }
        let t = c / (2.0 * b);
        return valid(t).then_some(t);
    }
    let discriminant = b * b - a * c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let (t0, t1) = ((b + root) / a, (b - root) / a);
    let (high, low) = if t0 >= t1 { (t0, t1) } else { (t1, t0) };
    if valid(high) {
        Some(high)
    } else if valid(low) {
        Some(low)
    } else {
        None
    }
}

fn composite(mode: CompositeMode, source: [f32; 4], dest: [f32; 4]) -> [f32; 4] {
    let (sa, da) = (source[3], dest[3]);
    if is_blend_mode(mode) {
        let unpremultiply = |pixel: [f32; 4]| -> [f32; 3] {
            if pixel[3] <= 0.0 {
                [0.0; 3]
            } else {
                [
                    pixel[0] / pixel[3],
                    pixel[1] / pixel[3],
                    pixel[2] / pixel[3],
                ]
            }
        };
        let blended = blend_colors(mode, unpremultiply(dest), unpremultiply(source));
        let mut out = [0.0; 4];
        for channel in 0..3 {
            out[channel] = source[channel] * (1.0 - da)
                + dest[channel] * (1.0 - sa)
                + sa * da * blended[channel];
        }
        out[3] = sa + da - sa * da;
        return out.map(|value| value.clamp(0.0, 1.0));
    }
    let (fs, fd) = match mode {
        CompositeMode::Clear => (0.0, 0.0),
        CompositeMode::Src => (1.0, 0.0),
        CompositeMode::Dest => (0.0, 1.0),
        CompositeMode::DestOver => (1.0 - da, 1.0),
        CompositeMode::SrcIn => (da, 0.0),
        CompositeMode::DestIn => (0.0, sa),
        CompositeMode::SrcOut => (1.0 - da, 0.0),
        CompositeMode::DestOut => (0.0, 1.0 - sa),
        CompositeMode::SrcAtop => (da, 1.0 - sa),
        CompositeMode::DestAtop => (1.0 - da, sa),
        CompositeMode::Xor => (1.0 - da, 1.0 - sa),
        CompositeMode::Plus => (1.0, 1.0),
        _ => (1.0, 1.0 - sa),
    };
    let mut out = [0.0; 4];
    for channel in 0..4 {
        out[channel] = (source[channel] * fs + dest[channel] * fd).min(1.0);
    }
    out
}

impl ColorPainter for ColorRaster<'_, '_> {
    fn push_transform(&mut self, transform: skrifa::color::Transform) {
        let local = Transform::new(
            transform.xx,
            transform.yx,
            transform.xy,
            transform.yy,
            transform.dx,
            transform.dy,
        );
        let next = compose(&local, &self.current());
        self.transforms.push(next);
    }

    fn pop_transform(&mut self) {
        self.transforms.pop();
    }

    fn push_clip_glyph(&mut self, glyph_id: GlyphId) {
        let commands = {
            let outline = self.font.outline_glyphs().get(glyph_id);
            outline.and_then(|outline| {
                let mut pen = CommandPen {
                    commands: Vec::new(),
                };
                outline
                    .draw(
                        DrawSettings::unhinted(
                            Size::unscaled(),
                            LocationRef::from(self.entry.location()),
                        ),
                        &mut pen,
                    )
                    .ok()
                    .map(|_| pen.commands)
            })
        };
        let mask = commands.map_or_else(
            || vec![0.0; self.width * self.height],
            |c| self.render_mask(&c),
        );
        let combined = match self.clips.last() {
            Some(previous) => previous.iter().zip(&mask).map(|(a, b)| a * b).collect(),
            None => mask,
        };
        self.clips.push(combined);
    }

    fn push_clip_box(&mut self, clip_box: BoundingBox<f32>) {
        let commands = [
            Command::MoveTo(Point::new(clip_box.x_min, clip_box.y_min)),
            Command::LineTo(Point::new(clip_box.x_max, clip_box.y_min)),
            Command::LineTo(Point::new(clip_box.x_max, clip_box.y_max)),
            Command::LineTo(Point::new(clip_box.x_min, clip_box.y_max)),
            Command::Close,
        ];
        let mask = self.render_mask(&commands);
        let combined = match self.clips.last() {
            Some(previous) => previous.iter().zip(&mask).map(|(a, b)| a * b).collect(),
            None => mask,
        };
        self.clips.push(combined);
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn fill(&mut self, brush: Brush<'_>) {
        let width = self.width;
        let height = self.height;
        let inverse = invert(&self.current());
        let clip = self.clips.last().cloned();
        let colors: Vec<[f32; 4]> = (0..width * height)
            .map(|index| {
                let coverage = clip.as_ref().map_or(1.0, |clip| clip[index]);
                if coverage <= 0.0 {
                    return [0.0; 4];
                }
                let (px, py) = ((index % width) as f32 + 0.5, (index / width) as f32 + 0.5);
                let local = inverse.map(|inv| {
                    (
                        inv.xx * px + inv.yx * py + inv.x,
                        inv.xy * px + inv.yy * py + inv.y,
                    )
                });
                let color = match (&brush, local) {
                    (
                        Brush::Solid {
                            palette_index,
                            alpha,
                        },
                        _,
                    ) => self.brush_color(*palette_index, *alpha),
                    (
                        Brush::LinearGradient {
                            p0,
                            p1,
                            color_stops,
                            extend,
                        },
                        Some((x, y)),
                    ) => {
                        let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                        let length = (dx * dx + dy * dy).max(1e-9);
                        let t = ((x - p0.x) * dx + (y - p0.y) * dy) / length;
                        self.stop_color(color_stops, t, *extend)
                    }
                    (
                        Brush::RadialGradient {
                            c0,
                            r0,
                            c1,
                            r1,
                            color_stops,
                            extend,
                        },
                        Some((x, y)),
                    ) => match radial_parameter((x, y), (c0.x, c0.y), *r0, (c1.x, c1.y), *r1) {
                        Some(t) => self.stop_color(color_stops, t, *extend),
                        None => [0.0; 4],
                    },
                    (
                        Brush::SweepGradient {
                            c0,
                            start_angle,
                            end_angle,
                            color_stops,
                            extend,
                        },
                        Some((x, y)),
                    ) => {
                        let angle = (-(y - c0.y).atan2(x - c0.x)).to_degrees().rem_euclid(360.0);
                        let span = (end_angle - start_angle).max(1e-6);
                        self.stop_color(color_stops, (angle - start_angle) / span, *extend)
                    }
                    _ => [0.0; 4],
                };
                let alpha = color[3] * coverage;
                [color[0] * alpha, color[1] * alpha, color[2] * alpha, alpha]
            })
            .collect();
        let canvas = self.canvas();
        for (pixel, color) in canvas.pixels.iter_mut().zip(colors) {
            *pixel = composite(CompositeMode::SrcOver, color, *pixel);
        }
    }

    fn push_layer(&mut self, composite_mode: CompositeMode) {
        let layer = Canvas::new(self.width, self.height);
        self.layers.push((layer, composite_mode));
    }

    fn pop_layer(&mut self) {
        if self.layers.len() <= 1 {
            return;
        }
        let (layer, mode) = self.layers.pop().expect("layer present");
        let clip = self.clips.last().cloned();
        let canvas = self.canvas();
        for (index, (pixel, source)) in canvas.pixels.iter_mut().zip(layer.pixels).enumerate() {
            let coverage = clip.as_ref().map_or(1.0, |clip| clip[index]);
            let source = source.map(|c| c * coverage);
            *pixel = composite(mode, source, *pixel);
        }
    }
}

fn rasterize_color(
    font: &skrifa::FontRef<'_>,
    entry: &FontEntry,
    glyph: GlyphId,
    size: f32,
    subpixel: u8,
) -> Option<Bitmap> {
    let color_glyph = font.color_glyphs().get(glyph)?;
    let palette: Vec<[f32; 4]> = font
        .color_palettes()
        .get(0)?
        .colors()
        .iter()
        .map(|color| {
            [
                f32::from(color.red) / 255.0,
                f32::from(color.green) / 255.0,
                f32::from(color.blue) / 255.0,
                f32::from(color.alpha) / 255.0,
            ]
        })
        .collect();
    let upem = entry.units_per_em();
    let scale = size / upem;
    let pad_left = (size * 0.5).ceil() as i32;
    let pad_right = (size * 1.75).ceil() as i32;
    let pad_down = (size * 0.75).ceil() as i32;
    let pad_up = (size * 1.5).ceil() as i32;
    let width = (pad_left + pad_right) as usize;
    let height = (pad_down + pad_up) as usize;
    let base = Transform::new(
        scale,
        0.0,
        0.0,
        -scale,
        pad_left as f32 + f32::from(subpixel) / f32::from(SUBPIXEL_STEPS),
        pad_up as f32,
    );
    let mut raster = ColorRaster {
        font,
        entry,
        palette,
        base,
        transforms: Vec::new(),
        clips: Vec::new(),
        layers: vec![(Canvas::new(width, height), CompositeMode::SrcOver)],
        width,
        height,
    };
    color_glyph
        .paint(LocationRef::from(entry.location()), &mut raster)
        .ok()?;
    let canvas = raster.layers.swap_remove(0).0;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (width, height, 0usize, 0usize);
    for y in 0..height {
        for x in 0..width {
            if canvas.pixels[y * width + x][3] > 0.0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x + 1);
                max_y = max_y.max(y + 1);
            }
        }
    }
    if min_x >= max_x || min_y >= max_y {
        return None;
    }
    let out_width = max_x - min_x;
    let out_height = max_y - min_y;
    let mut data = Vec::with_capacity(out_width * out_height * 4);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let pixel = canvas.pixels[y * width + x];
            for channel in pixel {
                data.push((channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    Some(Bitmap {
        left: min_x as i32 - pad_left,
        top: pad_up - min_y as i32,
        width: out_width as u32,
        height: out_height as u32,
        data,
    })
}
