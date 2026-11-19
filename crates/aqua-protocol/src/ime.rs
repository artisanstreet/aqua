use aqua_core::{ImeEvent as Composition, Point, Rect};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Utf16Range {
    pub start: u32,
    pub end: u32,
}

impl Utf16Range {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub const fn is_empty(self) -> bool {
        self.end <= self.start
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImeDocument {
    pub window_start: u32,
    pub total_len: u32,
    pub text: String,
    pub selection: Utf16Range,
    pub selection_reversed: bool,
    pub area: Rect,
    pub geometry: ImeGeometry,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImeGeometry {
    pub clip: Rect,
    pub lines: Vec<ImeLine>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImeLine {
    pub top: f32,
    pub bottom: f32,
    pub stops: Vec<ImeStop>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImeStop {
    pub offset: u32,
    pub x: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImeBounds {
    pub rect: Rect,
    pub clipped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImeHit {
    pub nearest: u32,
    pub inside: Option<u32>,
}

impl ImeGeometry {
    pub const UNAVAILABLE: Self = Self {
        clip: Rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        },
        lines: Vec::new(),
    };

    pub fn range_bounds(&self, range: Utf16Range) -> Option<ImeBounds> {
        let spans = self.lines.iter().filter_map(|line| {
            let xs = line
                .stops
                .iter()
                .filter(|stop| (range.start..=range.end).contains(&stop.offset));
            let covers = match range.is_empty() {
                true => xs.clone().next().is_some(),
                false => {
                    xs.clone().any(|stop| stop.offset < range.end)
                        && xs.clone().any(|stop| stop.offset > range.start)
                }
            };
            let left = xs.clone().map(|stop| stop.x).reduce(f32::min)?;
            let right = xs.map(|stop| stop.x).reduce(f32::max)?;
            covers.then(|| Rect::from_edges(left, line.top, right, line.bottom))
        });
        let spans: Vec<Rect> = match range.is_empty() {
            true => spans.take(1).collect(),
            false => spans.collect(),
        };
        let rect = spans.into_iter().reduce(|a, b| {
            Rect::from_edges(
                a.x.min(b.x),
                a.y.min(b.y),
                a.right().max(b.right()),
                a.bottom().max(b.bottom()),
            )
        })?;
        let clipped = rect.x < self.clip.x
            || rect.y < self.clip.y
            || rect.right() > self.clip.right()
            || rect.bottom() > self.clip.bottom();
        Some(ImeBounds { rect, clipped })
    }

    pub fn hit(&self, point: Point) -> Option<ImeHit> {
        let distance = |line: &ImeLine| match point.y {
            y if y < line.top => line.top - y,
            y if y >= line.bottom => y - line.bottom,
            _ => 0.0,
        };
        let line = self
            .lines
            .iter()
            .filter(|line| !line.stops.is_empty())
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
        let nearest = line
            .stops
            .iter()
            .min_by(|a, b| (a.x - point.x).abs().total_cmp(&(b.x - point.x).abs()))?
            .offset;
        let inside = line
            .stops
            .windows(2)
            .filter(|pair| {
                let (left, right) = (pair[0].x.min(pair[1].x), pair[0].x.max(pair[1].x));
                left <= point.x && point.x < right
            })
            .min_by(|a, b| (a[1].x - a[0].x).abs().total_cmp(&(b[1].x - b[0].x).abs()))
            .map(|pair| pair[0].offset)
            .filter(|_| distance(line) == 0.0);
        Some(ImeHit { nearest, inside })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImeCommand {
    Enable(ImeDocument),
    Update(ImeDocument),
    Disable,
    Reconvert,
    CommitComposition,
    CancelComposition,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImeEvent {
    Composition(Composition),
    Replace { range: Utf16Range, text: String },
    Select { range: Utf16Range, reversed: bool },
}

pub trait ImeBackend {
    type Error: std::error::Error;

    fn command(&mut self, command: ImeCommand) -> Result<(), Self::Error>;
    fn drain(&mut self, out: &mut Vec<ImeEvent>);
}
