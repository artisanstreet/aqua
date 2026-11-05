use aqua_core::Oklch;
use aqua_core::geometry::Rect;

use crate::linear::Linear2;

pub const STORE_WIDTH: u32 = 1024;
pub const HEADER_TEXELS: u32 = 4;
pub const CURVE_TEXELS: u32 = 2;
pub const LIST_LANES: u32 = 4;

const ROOT_CODE_TABLE: u32 = 0x2E74;
const CUBIC_TOLERANCE_EM: f32 = 1.0 / 8192.0;
const MAX_CUBIC_PIECES: usize = 16;
const BAND_TARGET_CURVES: usize = 4;
const MAX_BANDS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlugInstance {
    pub origin: [f32; 2],
    pub linear: Linear2,
    pub glyph: u32,
    pub em_bounds: [f32; 4],
    pub color: Oklch,
    pub clip: Option<Rect>,
    pub opacity: f32,
}

impl SlugInstance {
    pub fn em_to_screen(&self, point: [f32; 2]) -> [f32; 2] {
        let moved = self.linear.apply(point);
        [self.origin[0] + moved[0], self.origin[1] + moved[1]]
    }

    pub fn pixels_per_em(&self) -> f32 {
        self.linear.scale_factor()
    }

    pub fn covered(&self) -> Rect {
        let [x0, y0, x1, y1] = self.em_bounds;
        let corners =
            [[x0, y0], [x1, y0], [x0, y1], [x1, y1]].map(|corner| self.em_to_screen(corner));
        let (left, right, top, bottom) = corners.iter().fold(
            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
            |(left, right, top, bottom), corner| {
                (
                    left.min(corner[0]),
                    right.max(corner[0]),
                    top.min(corner[1]),
                    bottom.max(corner[1]),
                )
            },
        );
        Rect::from_edges(left - 1.0, top - 1.0, right + 1.0, bottom + 1.0)
    }
}

pub fn texel_position(index: u32) -> (u32, u32) {
    (index % STORE_WIDTH, index / STORE_WIDTH)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Accumulator {
    pub coverage: f32,
    pub weight: f32,
}

fn saturate(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

pub fn root_code(y1: f32, y2: f32, y3: f32) -> u32 {
    let shift = u32::from(y1.is_sign_negative())
        | (u32::from(y2.is_sign_negative()) << 1)
        | (u32::from(y3.is_sign_negative()) << 2);
    (ROOT_CODE_TABLE >> shift) & 0x0101
}

pub fn solve_axis(along: [f32; 3], across: [f32; 3]) -> [f32; 2] {
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

pub fn accumulate(
    curve: [[f32; 2]; 3],
    sample: [f32; 2],
    pixels_per_em: f32,
    horizontal: bool,
) -> Option<Accumulator> {
    let (along, across) = match horizontal {
        true => (1, 0),
        false => (0, 1),
    };
    let rel = |p: [f32; 2]| [p[0] - sample[0], p[1] - sample[1]];
    let (p1, p2, p3) = (rel(curve[0]), rel(curve[1]), rel(curve[2]));
    let farthest = p1[across].max(p2[across]).max(p3[across]);
    if farthest * pixels_per_em < -0.5 {
        return None;
    }
    let code = root_code(p1[along], p2[along], p3[along]);
    let mut out = Accumulator::default();
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

pub fn combine(across_x: Accumulator, across_y: Accumulator) -> f32 {
    let blended = (across_x.coverage * across_x.weight + across_y.coverage * across_y.weight).abs()
        / (across_x.weight + across_y.weight).max(1.0 / 65536.0);
    saturate(blended.max(across_x.coverage.abs().min(across_y.coverage.abs())))
}

fn scan(
    fetch: &impl Fn(u32) -> [f32; 4],
    glyph: u32,
    sample: [f32; 2],
    pixels_per_em: f32,
    horizontal: bool,
) -> Accumulator {
    let lists_at = fetch(glyph + 3)[0] as u32;
    let (origin, per_em, count, ranges_at, coordinate) = match horizontal {
        true => {
            let a = fetch(glyph + 1);
            let b = fetch(glyph + 2);
            (a[0], a[1], a[2], b[2], sample[1])
        }
        false => {
            let a = fetch(glyph + 1);
            let b = fetch(glyph + 2);
            (b[0], b[1], a[3], b[3], sample[0])
        }
    };
    let last = (count as u32).saturating_sub(1);
    let band = ((coordinate - origin) * per_em).floor();
    let band = match band < 0.0 {
        true => 0,
        false => (band as u32).min(last),
    };
    let range = fetch(ranges_at as u32 + band);
    let (start, members) = (range[0] as u32, range[1] as u32);
    let mut total = Accumulator::default();
    for slot in start..start + members {
        let lane = fetch(lists_at + slot / LIST_LANES);
        let curve_at = lane[(slot % LIST_LANES) as usize] as u32;
        let first = fetch(curve_at);
        let second = fetch(curve_at + 1);
        let curve = [
            [first[0], first[1]],
            [first[2], first[3]],
            [second[0], second[1]],
        ];
        let Some(part) = accumulate(curve, sample, pixels_per_em, horizontal) else {
            break;
        };
        total.coverage += part.coverage;
        total.weight = total.weight.max(part.weight);
    }
    total
}

pub fn coverage(
    fetch: impl Fn(u32) -> [f32; 4],
    glyph: u32,
    sample: [f32; 2],
    pixels_per_em: f32,
) -> f32 {
    let across_x = scan(&fetch, glyph, sample, pixels_per_em, true);
    let across_y = scan(&fetch, glyph, sample, pixels_per_em, false);
    combine(across_x, across_y)
}

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
            let mut total = Accumulator::default();
            let Some(range) = bands.band_of(coordinate) else {
                return total;
            };
            for index in bands.members(range) {
                let curve = &self.curves[*index as usize];
                let Some(part) = accumulate(
                    [curve.p1, curve.p2, curve.p3],
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
        combine(across_x, across_y)
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

pub fn pack_glyph(glyph: &SlugGlyph, base: u32) -> Vec<[f32; 4]> {
    let (horizontal, vertical) = (&glyph.horizontal, &glyph.vertical);
    let h_ranges_at = base + HEADER_TEXELS;
    let v_ranges_at = h_ranges_at + horizontal.ranges.len() as u32;
    let curves_at = v_ranges_at + vertical.ranges.len() as u32;
    let lists_at = curves_at + glyph.curves.len() as u32 * CURVE_TEXELS;
    let shift = horizontal.curves.len() as u32;
    let mut texels: Vec<[f32; 4]> = vec![
        glyph.bounds,
        [
            horizontal.origin,
            horizontal.bands_per_em,
            horizontal.ranges.len() as f32,
            vertical.ranges.len() as f32,
        ],
    ];
    texels.push([
        vertical.origin,
        vertical.bands_per_em,
        h_ranges_at as f32,
        v_ranges_at as f32,
    ]);
    texels.push([lists_at as f32, 0.0, 0.0, 0.0]);
    texels.extend(
        horizontal
            .ranges
            .iter()
            .map(|range| [range.offset as f32, range.count as f32, 0.0, 0.0]),
    );
    texels.extend(
        vertical
            .ranges
            .iter()
            .map(|range| [(range.offset + shift) as f32, range.count as f32, 0.0, 0.0]),
    );
    texels.extend(glyph.curves.iter().flat_map(|curve| {
        [
            [curve.p1[0], curve.p1[1], curve.p2[0], curve.p2[1]],
            [curve.p3[0], curve.p3[1], 0.0, 0.0],
        ]
    }));
    let members: Vec<f32> = horizontal
        .curves
        .iter()
        .chain(&vertical.curves)
        .map(|index| (curves_at + index * CURVE_TEXELS) as f32)
        .collect();
    texels.extend(members.chunks(LIST_LANES as usize).map(|lane| {
        let mut texel = [0.0; 4];
        texel[..lane.len()].copy_from_slice(lane);
        texel
    }));
    texels
}
