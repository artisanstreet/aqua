use std::collections::HashMap;
use std::sync::Arc;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{GlyphId, MetadataProvider};

use crate::font::{FontId, FontManager};
use crate::raster::RasterKey;
use crate::style::FontStretch;

const ITALIC_SKEW: f32 = 0.2126;
const CUBIC_TOLERANCE_EM: f32 = 1.0 / 8192.0;
const MAX_CUBIC_PIECES: usize = 16;
const BAND_TARGET_CURVES: usize = 4;
const MAX_BANDS: usize = 32;
const ROOT_CODE_TABLE: u32 = 0x2E74;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCommand {
    Move([f32; 2]),
    Line([f32; 2]),
    Quad([f32; 2], [f32; 2]),
    Cubic([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlugCurve {
    pub p1: [f32; 2],
    pub p2: [f32; 2],
    pub p3: [f32; 2],
}

impl SlugCurve {
    fn max_x(&self) -> f32 {
        self.p1[0].max(self.p2[0]).max(self.p3[0])
    }

    fn max_y(&self) -> f32 {
        self.p1[1].max(self.p2[1]).max(self.p3[1])
    }

    fn min_x(&self) -> f32 {
        self.p1[0].min(self.p2[0]).min(self.p3[0])
    }

    fn min_y(&self) -> f32 {
        self.p1[1].min(self.p2[1]).min(self.p3[1])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BandRange {
    pub offset: u32,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BandSet {
    pub origin: f32,
    pub bands_per_em: f32,
    pub ranges: Vec<BandRange>,
    pub curves: Vec<u32>,
}

impl BandSet {
    fn band_of(&self, coordinate: f32) -> Option<&BandRange> {
        let band = ((coordinate - self.origin) * self.bands_per_em).floor();
        match band {
            band if band < 0.0 => self.ranges.first(),
            band => self.ranges.get(band as usize).or(self.ranges.last()),
        }
    }

    fn members<'a>(&'a self, range: &BandRange) -> &'a [u32] {
        &self.curves[range.offset as usize..(range.offset + range.count) as usize]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlugGlyph {
    pub bounds: [f32; 4],
    pub curves: Vec<SlugCurve>,
    pub horizontal: BandSet,
    pub vertical: BandSet,
}

fn midpoint(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

fn cubic_point(p: [[f32; 2]; 4], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let weights = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    let mut out = [0.0; 2];
    for (weight, point) in weights.iter().zip(p) {
        out[0] += weight * point[0];
        out[1] += weight * point[1];
    }
    out
}

fn cubic_to_quads(
    p0: [f32; 2],
    p1: [f32; 2],
    p2: [f32; 2],
    p3: [f32; 2],
    out: &mut Vec<SlugCurve>,
) {
    let deviation = [
        p3[0] - 3.0 * p2[0] + 3.0 * p1[0] - p0[0],
        p3[1] - 3.0 * p2[1] + 3.0 * p1[1] - p0[1],
    ];
    let magnitude = (deviation[0] * deviation[0] + deviation[1] * deviation[1]).sqrt();
    let error = magnitude * 3.0f32.sqrt() / 36.0;
    let pieces = ((error / CUBIC_TOLERANCE_EM).cbrt().ceil() as usize).clamp(1, MAX_CUBIC_PIECES);
    let controls = [p0, p1, p2, p3];
    for piece in 0..pieces {
        let (t0, t1) = (
            piece as f32 / pieces as f32,
            (piece + 1) as f32 / pieces as f32,
        );
        let (start, end) = (cubic_point(controls, t0), cubic_point(controls, t1));
        let span = t1 - t0;
        let tangent = |t: f32| {
            let u = 1.0 - t;
            let derivative = |axis: usize| {
                3.0 * (u * u * (p1[axis] - p0[axis])
                    + 2.0 * u * t * (p2[axis] - p1[axis])
                    + t * t * (p3[axis] - p2[axis]))
            };
            [derivative(0) * span, derivative(1) * span]
        };
        let (ts, te) = (tangent(t0), tangent(t1));
        let from_start = [start[0] + ts[0] / 3.0, start[1] + ts[1] / 3.0];
        let from_end = [end[0] - te[0] / 3.0, end[1] - te[1] / 3.0];
        let control = midpoint(from_start, from_end);
        out.push(SlugCurve {
            p1: start,
            p2: control,
            p3: end,
        });
    }
}

fn line_curve(from: [f32; 2], to: [f32; 2]) -> SlugCurve {
    SlugCurve {
        p1: from,
        p2: midpoint(from, to),
        p3: to,
    }
}

fn flatten(commands: &[PathCommand]) -> Vec<SlugCurve> {
    let mut curves = Vec::new();
    let (mut start, mut current) = ([0.0f32; 2], [0.0f32; 2]);
    for command in commands {
        match *command {
            PathCommand::Move(to) => {
                if current != start {
                    curves.push(line_curve(current, start));
                }
                (start, current) = (to, to);
            }
            PathCommand::Line(to) => {
                if to != current {
                    curves.push(line_curve(current, to));
                }
                current = to;
            }
            PathCommand::Quad(control, to) => {
                curves.push(SlugCurve {
                    p1: current,
                    p2: control,
                    p3: to,
                });
                current = to;
            }
            PathCommand::Cubic(c1, c2, to) => {
                cubic_to_quads(current, c1, c2, to, &mut curves);
                current = to;
            }
            PathCommand::Close => {
                if current != start {
                    curves.push(line_curve(current, start));
                }
                current = start;
            }
        }
    }
    if current != start {
        curves.push(line_curve(current, start));
    }
    curves
}

fn build_bands(
    curves: &[SlugCurve],
    low: f32,
    high: f32,
    extent: impl Fn(&SlugCurve) -> (f32, f32),
    order: impl Fn(&SlugCurve) -> f32,
) -> BandSet {
    let span = (high - low).max(f32::EPSILON);
    let count = (curves.len() / BAND_TARGET_CURVES).clamp(1, MAX_BANDS);
    let bands_per_em = count as f32 / span;
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); count];
    for (index, curve) in curves.iter().enumerate() {
        let (min, max) = extent(curve);
        let first = (((min - low) * bands_per_em).floor().max(0.0) as usize).min(count - 1);
        let last = (((max - low) * bands_per_em).floor().max(0.0) as usize).min(count - 1);
        for band in &mut members[first..=last] {
            band.push(index as u32);
        }
    }
    let mut ranges = Vec::with_capacity(count);
    let mut flat = Vec::new();
    for mut band in members {
        band.sort_by(|a, b| {
            order(&curves[*b as usize])
                .partial_cmp(&order(&curves[*a as usize]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranges.push(BandRange {
            offset: flat.len() as u32,
            count: band.len() as u32,
        });
        flat.extend(band);
    }
    BandSet {
        origin: low,
        bands_per_em,
        ranges,
        curves: flat,
    }
}

fn saturate(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn root_code(y1: f32, y2: f32, y3: f32) -> u32 {
    let shift = u32::from(y1.is_sign_negative())
        | (u32::from(y2.is_sign_negative()) << 1)
        | (u32::from(y3.is_sign_negative()) << 2);
    (ROOT_CODE_TABLE >> shift) & 0x0101
}

fn solve_axis(along: [f32; 3], across: [f32; 3]) -> [f32; 2] {
    let a_across = along[0] - 2.0 * along[1] + along[2];
    let b_across = along[0] - along[1];
    let (t1, t2) = match a_across.abs() < 1.0 / 65536.0 {
        true => {
            let t = along[0] / (2.0 * b_across);
            (t, t)
        }
        false => {
            let discriminant = (b_across * b_across - a_across * along[0]).max(0.0).sqrt();
            let reciprocal = 1.0 / a_across;
            (
                (b_across - discriminant) * reciprocal,
                (b_across + discriminant) * reciprocal,
            )
        }
    };
    let a = across[0] - 2.0 * across[1] + across[2];
    let b = across[0] - across[1];
    let at = |t: f32| (a * t - b * 2.0) * t + across[0];
    [at(t1), at(t2)]
}

struct Accumulator {
    coverage: f32,
    weight: f32,
}

fn accumulate(
    curve: &SlugCurve,
    sample: [f32; 2],
    pixels_per_em: f32,
    horizontal: bool,
) -> Option<Accumulator> {
    let (along, across) = match horizontal {
        true => (1, 0),
        false => (0, 1),
    };
    let rel = |p: [f32; 2]| [p[0] - sample[0], p[1] - sample[1]];
    let (p1, p2, p3) = (rel(curve.p1), rel(curve.p2), rel(curve.p3));
    let farthest = p1[across].max(p2[across]).max(p3[across]);
    if farthest * pixels_per_em < -0.5 {
        return None;
    }
    let code = root_code(p1[along], p2[along], p3[along]);
    let mut out = Accumulator {
        coverage: 0.0,
        weight: 0.0,
    };
    if code != 0 {
        let roots = solve_axis(
            [p1[along], p2[along], p3[along]],
            [p1[across], p2[across], p3[across]],
        );
        let scaled = roots.map(|root| root * pixels_per_em);
        if code & 1 != 0 {
            out.coverage += saturate(scaled[0] + 0.5);
            out.weight = out.weight.max(saturate(1.0 - scaled[0].abs() * 2.0));
        }
        if code > 1 {
            out.coverage -= saturate(scaled[1] + 0.5);
            out.weight = out.weight.max(saturate(1.0 - scaled[1].abs() * 2.0));
        }
    }
    Some(out)
}

impl SlugGlyph {
    pub fn from_path(commands: &[PathCommand]) -> Option<Self> {
        let curves = flatten(commands);
        if curves.is_empty() {
            return None;
        }
        let bounds =
            curves
                .iter()
                .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |bounds, curve| {
                    [
                        bounds[0].min(curve.min_x()),
                        bounds[1].min(curve.min_y()),
                        bounds[2].max(curve.max_x()),
                        bounds[3].max(curve.max_y()),
                    ]
                });
        let horizontal = build_bands(
            &curves,
            bounds[1],
            bounds[3],
            |curve| (curve.min_y(), curve.max_y()),
            SlugCurve::max_x,
        );
        let vertical = build_bands(
            &curves,
            bounds[0],
            bounds[2],
            |curve| (curve.min_x(), curve.max_x()),
            SlugCurve::max_y,
        );
        Some(Self {
            bounds,
            curves,
            horizontal,
            vertical,
        })
    }

    pub fn coverage(&self, x: f32, y: f32, pixels_per_em: f32) -> f32 {
        let sample = [x, y];
        let scan = |bands: &BandSet, coordinate: f32, horizontal: bool| {
            let mut total = Accumulator {
                coverage: 0.0,
                weight: 0.0,
            };
            let Some(range) = bands.band_of(coordinate) else {
                return total;
            };
            for index in bands.members(range) {
                let Some(part) = accumulate(
                    &self.curves[*index as usize],
                    sample,
                    pixels_per_em,
                    horizontal,
                ) else {
                    break;
                };
                total.coverage += part.coverage;
                total.weight = total.weight.max(part.weight);
            }
            total
        };
        let across_x = scan(&self.horizontal, y, true);
        let across_y = scan(&self.vertical, x, false);
        let blended = (across_x.coverage * across_x.weight + across_y.coverage * across_y.weight)
            .abs()
            / (across_x.weight + across_y.weight).max(1.0 / 65536.0);
        saturate(blended.max(across_x.coverage.abs().min(across_y.coverage.abs())))
    }

    pub fn texels(&self) -> Vec<[f32; 4]> {
        self.curves
            .iter()
            .flat_map(|curve| {
                [
                    [curve.p1[0], curve.p1[1], curve.p2[0], curve.p2[1]],
                    [curve.p3[0], curve.p3[1], 0.0, 0.0],
                ]
            })
            .collect()
    }

    pub fn heap_bytes(&self) -> usize {
        self.curves.len() * std::mem::size_of::<SlugCurve>()
            + (self.horizontal.curves.len() + self.vertical.curves.len()) * 4
            + (self.horizontal.ranges.len() + self.vertical.ranges.len()) * 8
    }
}

struct EmPen {
    scale: f32,
    skew: f32,
    commands: Vec<PathCommand>,
}

impl EmPen {
    fn map(&self, x: f32, y: f32) -> [f32; 2] {
        [(x + self.skew * y) * self.scale, y * self.scale]
    }
}

impl OutlinePen for EmPen {
    fn move_to(&mut self, x: f32, y: f32) {
        let point = self.map(x, y);
        self.commands.push(PathCommand::Move(point));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let point = self.map(x, y);
        self.commands.push(PathCommand::Line(point));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (control, point) = (self.map(cx0, cy0), self.map(x, y));
        self.commands.push(PathCommand::Quad(control, point));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, point) = (self.map(cx0, cy0), self.map(cx1, cy1), self.map(x, y));
        self.commands.push(PathCommand::Cubic(c0, c1, point));
    }

    fn close(&mut self) {
        self.commands.push(PathCommand::Close);
    }
}

#[derive(Clone, Debug)]
pub enum SlugLookup {
    Outline(Arc<SlugGlyph>),
    Blank,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SlugKey {
    font_id: FontId,
    weight: u16,
    stretch: FontStretch,
    synthetic_italic: bool,
    glyph_id: u16,
}

#[derive(Default)]
pub struct SlugCache {
    glyphs: HashMap<SlugKey, SlugLookup>,
    revision: u64,
}

impl SlugCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    pub fn heap_bytes(&self) -> usize {
        self.glyphs
            .values()
            .map(|lookup| match lookup {
                SlugLookup::Outline(glyph) => glyph.heap_bytes(),
                _ => 0,
            })
            .sum()
    }

    pub fn clear(&mut self) {
        self.glyphs.clear();
    }

    pub fn lookup(&mut self, fonts: &mut FontManager, key: &RasterKey) -> SlugLookup {
        if self.revision != fonts.font_revision() {
            self.glyphs.clear();
            self.revision = fonts.font_revision();
        }
        let slug_key = SlugKey {
            font_id: key.font_id,
            weight: key.weight,
            stretch: key.stretch,
            synthetic_italic: key.synthetic_italic,
            glyph_id: key.glyph_id,
        };
        if let Some(found) = self.glyphs.get(&slug_key) {
            return found.clone();
        }
        let built = build(fonts, key);
        self.glyphs.insert(slug_key, built.clone());
        built
    }
}

fn build(fonts: &mut FontManager, key: &RasterKey) -> SlugLookup {
    let Some(entry) = fonts.entry_with(key.font_id, key.weight, key.stretch, key.synthetic_italic)
    else {
        return SlugLookup::Unsupported;
    };
    if entry.synthetic_bold() || entry.has_color() {
        return SlugLookup::Unsupported;
    }
    let scale = 1.0 / entry.units_per_em();
    let drawn = entry.with_font(|font| {
        let outline = font
            .outline_glyphs()
            .get(GlyphId::new(u32::from(key.glyph_id)))?;
        let mut pen = EmPen {
            scale,
            skew: if key.synthetic_italic {
                ITALIC_SKEW
            } else {
                0.0
            },
            commands: Vec::new(),
        };
        outline
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::from(entry.location())),
                &mut pen,
            )
            .ok()?;
        Some(pen.commands)
    });
    match drawn.flatten() {
        None => SlugLookup::Unsupported,
        Some(commands) => match SlugGlyph::from_path(&commands) {
            Some(glyph) => SlugLookup::Outline(Arc::new(glyph)),
            None => SlugLookup::Blank,
        },
    }
}
