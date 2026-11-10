use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use aqua_core::geometry::Rect;
use aqua_core::vector::VectorSource;
use aqua_core::vector::{
    Brush, Drawing, GpuiPath, GpuiPathStyle, GradientStop, PaintedPath, PathCommand, Stroke,
};
use aqua_core::{GpuiBackground, Oklch, Point, Transform2D};

use super::color;

const CACHE_LIMIT: usize = 256;
const TILE_CACHE_BYTES: u64 = 8 << 20;
const DRAWING_CACHE_BYTES: u64 = 16 << 20;
const SVG_CACHE_BYTES: u64 = 8 << 20;
const TILE_CACHE_ENTRIES: usize = 32;
const ROTATION_STEPS_PER_TURN: f32 = 120.0;
const SOURCE_FALLBACK_EDGE: f32 = 24.0;
const PATH_PADDING: f32 = 1.0;

#[derive(Clone, Debug)]
pub struct PathRaster {
    pub source: VectorSource,
    pub placement: Rect,
    pub width: u32,
    pub height: u32,
    pub geometry_key: u64,
}

impl PathRaster {
    pub fn tiles(self) -> Vec<Self> {
        if self.width <= 2048 && self.height <= 2048 {
            return vec![self];
        }
        let VectorSource::Drawing(drawing) = &self.source else {
            return vec![self];
        };
        let mut tiles = Vec::new();
        for top in (0..self.height).step_by(1024) {
            for left in (0..self.width).step_by(1024) {
                let width = (self.width - left).min(1024);
                let height = (self.height - top).min(1024);
                tiles.push(Self {
                    source: VectorSource::Drawing(Arc::new(Drawing {
                        view_box: Rect {
                            x: drawing.view_box.x + left as f32,
                            y: drawing.view_box.y + top as f32,
                            w: width as f32,
                            h: height as f32,
                        },
                        paths: drawing.paths.clone(),
                    })),
                    placement: Rect {
                        x: self.placement.x + left as f32,
                        y: self.placement.y + top as f32,
                        w: width as f32,
                        h: height as f32,
                    },
                    width,
                    height,
                    geometry_key: self.geometry_key,
                });
            }
        }
        tiles
    }
}

type SvgKey = (Arc<str>, Option<[u8; 4]>, i32);

#[derive(Debug)]
struct TileEntry {
    source: Arc<Drawing>,
    rasters: Vec<PathRaster>,
    accessed: u64,
    bytes: u64,
}

#[derive(Debug)]
struct CurveEntry {
    _source: VectorSource,
    scene: Option<Arc<aqua_media::vector::CurveScene>>,
    key: u64,
    accessed: u64,
    bytes: usize,
}

#[derive(Debug, Default)]
pub struct VectorCache {
    svgs: HashMap<SvgKey, VectorSource>,
    drawings: HashMap<u64, (Arc<Drawing>, u64, u64)>,
    drawing_bytes: u64,
    svg_bytes: u64,
    access: u64,
    tiles: HashMap<(usize, u32, u32), TileEntry>,
    tile_bytes: u64,
    curves: HashMap<usize, CurveEntry>,
    curve_bytes: usize,
}

#[must_use]
pub fn rotation_steps(radians: f32) -> i32 {
    match radians.is_finite() {
        true => (radians / std::f32::consts::TAU * ROTATION_STEPS_PER_TURN)
            .round()
            .rem_euclid(ROTATION_STEPS_PER_TURN) as i32,
        false => 0,
    }
}

fn root_extent(source: &str) -> (f32, f32) {
    let tag = source
        .find("<svg")
        .and_then(|start| {
            source[start..]
                .find('>')
                .map(|end| &source[start..start + end])
        })
        .unwrap_or("");
    let attribute = |name: &str| {
        let key = format!("{name}=\"");
        tag.match_indices(&key)
            .find(|(at, _)| {
                *at == 0
                    || !tag[..*at]
                        .chars()
                        .next_back()
                        .is_some_and(|before| before.is_alphanumeric() || before == '-')
            })
            .and_then(|(at, _)| {
                let rest = &tag[at + key.len()..];
                rest.find('"').map(|end| rest[..end].to_owned())
            })
    };
    let numbers = |value: String| {
        value
            .split([' ', ','])
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.trim_end_matches("px").parse::<f32>().ok())
            .collect::<Vec<_>>()
    };
    match attribute("viewBox").map(numbers).as_deref() {
        Some([_, _, w, h]) if *w > 0.0 && *h > 0.0 => (*w, *h),
        _ => match (
            attribute("width")
                .map(numbers)
                .and_then(|v| v.first().copied()),
            attribute("height")
                .map(numbers)
                .and_then(|v| v.first().copied()),
        ) {
            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => (w, h),
            _ => (SOURCE_FALLBACK_EDGE, SOURCE_FALLBACK_EDGE),
        },
    }
}

#[must_use]
pub fn rotated_svg(source: &str, steps: i32) -> String {
    let start = source.find("<svg").unwrap_or(0);
    let body = &source[start..];
    let (width, height) = root_extent(body);
    let degrees = steps as f32 * 360.0 / ROTATION_STEPS_PER_TURN;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width} {height}\" width=\"{width}\" height=\"{height}\"><g transform=\"rotate({degrees} {} {})\">{body}</g></svg>",
        width * 0.5,
        height * 0.5,
    )
}

struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        bytes.iter().for_each(|byte| {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        });
    }
}

impl std::fmt::Write for Fnv {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        self.write(value.as_bytes());
        Ok(())
    }
}

fn hash_float(state: &mut Fnv, value: f32) {
    value.to_bits().hash(state);
}

fn hash_point(state: &mut Fnv, point: Point) {
    hash_float(state, point.x);
    hash_float(state, point.y);
}

fn hash_command(state: &mut Fnv, command: &PathCommand) {
    match command {
        PathCommand::Move(to) => {
            0u8.hash(state);
            hash_point(state, *to);
        }
        PathCommand::Line(to) => {
            1u8.hash(state);
            hash_point(state, *to);
        }
        PathCommand::Quadratic { control, to } => {
            2u8.hash(state);
            hash_point(state, *control);
            hash_point(state, *to);
        }
        PathCommand::Cubic { first, second, to } => {
            3u8.hash(state);
            hash_point(state, *first);
            hash_point(state, *second);
            hash_point(state, *to);
        }
        PathCommand::Arc {
            radii,
            rotation_radians,
            large,
            sweep,
            to,
        } => {
            4u8.hash(state);
            hash_point(state, *radii);
            hash_float(state, *rotation_radians);
            large.hash(state);
            sweep.hash(state);
            hash_point(state, *to);
        }
        PathCommand::Close => 5u8.hash(state),
    }
}

fn map_command(command: &PathCommand, map: &impl Fn(Point) -> Point, scale: f32) -> PathCommand {
    match command {
        PathCommand::Move(to) => PathCommand::Move(map(*to)),
        PathCommand::Line(to) => PathCommand::Line(map(*to)),
        PathCommand::Quadratic { control, to } => PathCommand::Quadratic {
            control: map(*control),
            to: map(*to),
        },
        PathCommand::Cubic { first, second, to } => PathCommand::Cubic {
            first: map(*first),
            second: map(*second),
            to: map(*to),
        },
        PathCommand::Arc {
            radii,
            rotation_radians,
            large,
            sweep,
            to,
        } => PathCommand::Arc {
            radii: Point::new(radii.x * scale, radii.y * scale),
            rotation_radians: *rotation_radians,
            large: *large,
            sweep: *sweep,
            to: map(*to),
        },
        PathCommand::Close => PathCommand::Close,
    }
}

fn extent_points(command: &PathCommand) -> impl Iterator<Item = Point> {
    match command {
        PathCommand::Move(to) | PathCommand::Line(to) => [Some(*to), None, None],
        PathCommand::Quadratic { control, to } => [Some(*control), Some(*to), None],
        PathCommand::Cubic { first, second, to } => [Some(*first), Some(*second), Some(*to)],
        PathCommand::Arc { to, .. } => [Some(*to), None, None],
        PathCommand::Close => [None, None, None],
    }
    .into_iter()
    .flatten()
}

fn arc_reach(commands: &[PathCommand], scale: f32) -> f32 {
    commands
        .iter()
        .filter_map(|command| match command {
            PathCommand::Arc { radii, .. } => Some(radii.x.max(radii.y) * scale),
            _ => None,
        })
        .fold(0.0, f32::max)
}

#[must_use]
pub fn gradient_axis(bounds: Rect, angle_degrees: f32) -> (Point, Point) {
    let radians = angle_degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    let center = bounds.center();
    let length = (bounds.w * sin).abs() + (bounds.h * cos).abs();
    let half = length * 0.5;
    (
        Point::new(center.x - sin * half, center.y + cos * half),
        Point::new(center.x + sin * half, center.y - cos * half),
    )
}

fn brush(background: GpuiBackground, bounds: Rect) -> Brush {
    match background {
        GpuiBackground::Solid(color) => Brush::Solid(color::oklch(color::hsla(color))),
        GpuiBackground::LinearGradient { angle, stops, .. } => {
            let (start, end) = gradient_axis(bounds, angle);
            Brush::Linear {
                start,
                end,
                stops: stops
                    .iter()
                    .map(|stop| GradientStop {
                        position: stop.percentage,
                        color: color::oklch(color::hsla(stop.color)),
                    })
                    .collect::<Vec<_>>()
                    .into(),
            }
        }
    }
}

impl VectorCache {
    pub fn tiles(&mut self, raster: PathRaster) -> Vec<PathRaster> {
        if raster.width <= 2048 && raster.height <= 2048 {
            return vec![raster];
        }
        let VectorSource::Drawing(drawing) = &raster.source else {
            return vec![raster];
        };
        let key = (Arc::as_ptr(drawing) as usize, raster.width, raster.height);
        let origin = raster.placement;
        self.access = self.access.wrapping_add(1);
        if !self.tiles.contains_key(&key) {
            let source = Arc::clone(drawing);
            let rasters = raster.tiles();
            let bytes = aqua_media::vector::retained_source_bytes(&VectorSource::Drawing(
                Arc::clone(&source),
            ))
            .saturating_add(
                rasters
                    .iter()
                    .map(|tile| aqua_media::vector::retained_source_bytes(&tile.source))
                    .sum::<u64>(),
            );
            if bytes > TILE_CACHE_BYTES {
                return rasters;
            }
            while self.tiles.len() >= TILE_CACHE_ENTRIES
                || self.tile_bytes + bytes > TILE_CACHE_BYTES
            {
                let oldest = self
                    .tiles
                    .iter()
                    .min_by_key(|(_, entry)| entry.accessed)
                    .map(|(key, _)| *key);
                let Some(oldest) = oldest else { break };
                if let Some(entry) = self.tiles.remove(&oldest) {
                    self.tile_bytes -= entry.bytes;
                }
            }
            self.tiles.insert(
                key,
                TileEntry {
                    source,
                    rasters,
                    accessed: self.access,
                    bytes,
                },
            );
            self.tile_bytes += bytes;
        }
        let entry = self
            .tiles
            .get_mut(&key)
            .expect("the tile cache entry was inserted");
        entry.accessed = self.access;
        let tiles = &entry.rasters;
        let dx = origin.x - tiles[0].placement.x;
        let dy = origin.y - tiles[0].placement.y;
        tiles
            .iter()
            .cloned()
            .map(|mut tile| {
                tile.placement = tile.placement.translate(dx, dy);
                tile
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.svgs.clear();
        self.drawings.clear();
        self.drawing_bytes = 0;
        self.svg_bytes = 0;
        self.tiles.clear();
        self.tile_bytes = 0;
        self.curves.clear();
        self.curve_bytes = 0;
    }

    pub fn source_cache_bytes(&self) -> u64 {
        self.drawing_bytes + self.svg_bytes
    }

    fn remove_drawing(&mut self, key: u64) {
        if let Some((drawing, _, bytes)) = self.drawings.remove(&key) {
            self.drawing_bytes -= bytes;
            let identity = Arc::as_ptr(&drawing) as usize;
            self.tiles.retain(|_, entry| {
                let keep = Arc::as_ptr(&entry.source) as usize != identity;
                if !keep {
                    self.tile_bytes -= entry.bytes;
                }
                keep
            });
        }
    }

    pub fn tile_cache_bytes(&self) -> u64 {
        self.tile_bytes
    }

    pub fn curve_cache_bytes(&self) -> usize {
        self.curve_bytes
    }

    pub fn curves(
        &mut self,
        source: &VectorSource,
    ) -> Option<(u64, Arc<aqua_media::vector::CurveScene>)> {
        let identity = match source {
            VectorSource::Svg(svg) => Arc::as_ptr(svg) as usize,
            VectorSource::Drawing(drawing) => Arc::as_ptr(drawing) as usize,
        };
        self.access = self.access.wrapping_add(1);
        if let Some(entry) = self.curves.get_mut(&identity) {
            entry.accessed = self.access;
            return entry
                .scene
                .as_ref()
                .map(|scene| (entry.key, Arc::clone(scene)));
        }
        let budget = aqua_media::vector::VectorBudget::default();
        let built = match source {
            VectorSource::Svg(svg) => aqua_media::vector::svg_curves(svg, budget),
            VectorSource::Drawing(drawing) => aqua_media::vector::drawing_curves(drawing, budget),
        }
        .ok()
        .flatten()
        .map(Arc::new);
        let mut state = Fnv(0x91e10da5c79e7b1d);
        match source {
            VectorSource::Svg(svg) => {
                svg.source.hash(&mut state);
                svg.tint.map(Oklch::to_srgb8).hash(&mut state);
            }
            VectorSource::Drawing(drawing) => {
                for value in [
                    drawing.view_box.x,
                    drawing.view_box.y,
                    drawing.view_box.w,
                    drawing.view_box.h,
                ] {
                    hash_float(&mut state, value);
                }
                for path in &drawing.paths {
                    path.commands
                        .iter()
                        .for_each(|command| hash_command(&mut state, command));
                    format!(
                        "{:?}{:?}{:?}{:?}",
                        path.fill, path.stroke, path.fill_rule, path.transform
                    )
                    .hash(&mut state);
                }
            }
        }
        let key = state.finish();
        let bytes = aqua_media::vector::retained_source_bytes(source) as usize
            + built.as_ref().map_or(0, |scene| scene.heap_bytes())
            + size_of::<CurveEntry>();
        if bytes <= TILE_CACHE_BYTES as usize {
            while self.curves.len() >= 64 || self.curve_bytes + bytes > TILE_CACHE_BYTES as usize {
                let oldest = self
                    .curves
                    .iter()
                    .min_by_key(|(_, entry)| entry.accessed)
                    .map(|(id, _)| *id);
                let Some(entry) = oldest.and_then(|id| self.curves.remove(&id)) else {
                    break;
                };
                self.curve_bytes -= entry.bytes;
            }
            self.curve_bytes += bytes;
            self.curves.insert(
                identity,
                CurveEntry {
                    _source: source.clone(),
                    scene: built.clone(),
                    key,
                    accessed: self.access,
                    bytes,
                },
            );
        }
        built.map(|scene| (key, scene))
    }

    pub fn svg(&mut self, source: &Arc<str>, tint: Option<Oklch>, steps: i32) -> VectorSource {
        let key = (source.clone(), tint.map(Oklch::to_srgb8), steps);
        if let Some(found) = self.svgs.get(&key) {
            return found.clone();
        }
        let text: Arc<str> = match steps {
            0 => source.clone(),
            steps => rotated_svg(source, steps).into(),
        };
        let made = match tint {
            Some(tint) => VectorSource::tinted_svg(text, tint),
            None => VectorSource::svg(text),
        };
        let bytes = source.len() as u64 + aqua_media::vector::retained_source_bytes(&made);
        if bytes <= SVG_CACHE_BYTES {
            if self.svgs.len() >= CACHE_LIMIT || self.svg_bytes + bytes > SVG_CACHE_BYTES {
                self.svgs.clear();
                self.svg_bytes = 0;
            }
            self.svg_bytes += bytes;
            self.svgs.insert(key, made.clone());
        }
        made
    }

    pub fn path(&mut self, path: &GpuiPath, device: Transform2D) -> Option<PathRaster> {
        let to_device = |local: Point| {
            let window = path.window_logical_position(local);
            device.apply(window)
        };
        let scale = (device.a * device.d - device.b * device.c).abs().sqrt();
        let stroke_width = match &path.style {
            GpuiPathStyle::Stroke { width, .. } => width * scale,
            GpuiPathStyle::Fill { .. } => 0.0,
        };
        let mut points = path
            .commands_local
            .iter()
            .flat_map(extent_points)
            .map(to_device);
        let first = points.next()?;
        let (min, max) = points.fold(
            ((first.x, first.y), (first.x, first.y)),
            |((lx, ly), (hx, hy)), point| {
                (
                    (lx.min(point.x), ly.min(point.y)),
                    (hx.max(point.x), hy.max(point.y)),
                )
            },
        );
        let reach =
            arc_reach(&path.commands_local, scale) + stroke_width * 0.5 * 1.5 + PATH_PADDING;
        let origin = Point::new((min.0 - reach).floor(), (min.1 - reach).floor());
        let width = (max.0 + reach - origin.x).ceil().max(1.0);
        let height = (max.1 + reach - origin.y).ceil().max(1.0);
        match width * height > 16_777_216.0 || !width.is_finite() || !height.is_finite() {
            true => None,
            false => {
                let relative = |local: Point| {
                    let moved = to_device(local);
                    Point::new(moved.x - origin.x, moved.y - origin.y)
                };
                let local_bounds = Rect {
                    x: min.0 - origin.x,
                    y: min.1 - origin.y,
                    w: max.0 - min.0,
                    h: max.1 - min.1,
                };
                let mut state = Fnv(0xcbf2_9ce4_8422_2325);
                path.commands_local.iter().for_each(|command| {
                    let mapped = map_command(command, &relative, scale);
                    hash_command(&mut state, &mapped);
                });
                let _ = write!(state, "{:?}{:?}", path.style, path.background);
                state.write_u8(0xff);
                hash_float(&mut state, width);
                hash_float(&mut state, height);
                hash_float(&mut state, scale);
                let key = state.finish();
                self.access = self.access.wrapping_add(1);
                let drawing = match self.drawings.get_mut(&key) {
                    Some((found, accessed, _)) => {
                        *accessed = self.access;
                        found.clone()
                    }
                    None => {
                        let commands = path
                            .commands_local
                            .iter()
                            .map(|command| map_command(command, &relative, scale))
                            .collect();
                        let paint = brush(path.background, local_bounds);
                        let painted = match &path.style {
                            GpuiPathStyle::Fill { rule } => {
                                let mut fill = PaintedPath::fill(commands, paint);
                                fill.fill_rule = *rule;
                                fill
                            }
                            GpuiPathStyle::Stroke {
                                width,
                                cap,
                                join,
                                dash,
                                dash_offset,
                            } => {
                                let mut stroke = Stroke::new(paint, width * scale);
                                stroke.cap = *cap;
                                stroke.join = *join;
                                stroke.dash = dash.iter().map(|part| part * scale).collect();
                                stroke.dash_offset = dash_offset * scale;
                                PaintedPath::stroke(commands, stroke)
                            }
                        };
                        let made = Arc::new(Drawing {
                            view_box: Rect {
                                x: 0.0,
                                y: 0.0,
                                w: width,
                                h: height,
                            },
                            paths: vec![painted],
                        });
                        let bytes = aqua_media::vector::retained_source_bytes(
                            &VectorSource::Drawing(Arc::clone(&made)),
                        );
                        if bytes <= DRAWING_CACHE_BYTES {
                            while self.drawings.len() >= CACHE_LIMIT
                                || self.drawing_bytes + bytes > DRAWING_CACHE_BYTES
                            {
                                let oldest = self
                                    .drawings
                                    .iter()
                                    .min_by_key(|(_, (_, accessed, _))| *accessed)
                                    .map(|(id, _)| *id);
                                let Some(oldest) = oldest else {
                                    break;
                                };
                                self.remove_drawing(oldest);
                            }
                            self.drawing_bytes += bytes;
                            self.drawings
                                .insert(key, (made.clone(), self.access, bytes));
                        }
                        made
                    }
                };
                Some(PathRaster {
                    source: VectorSource::Drawing(drawing),
                    placement: Rect {
                        x: origin.x,
                        y: origin.y,
                        w: width,
                        h: height,
                    },
                    width: width as u32,
                    height: height as u32,
                    geometry_key: key,
                })
            }
        }
    }
}
