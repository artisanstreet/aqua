use std::collections::VecDeque;

use aqua_core::geometry::Rect;
use aqua_graph::Extent;

const MAX_RECTS: usize = 16;
const HISTORY: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PixelRect {
    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn union(self, other: PixelRect) -> PixelRect {
        PixelRect {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    pub fn area(&self) -> i64 {
        match self.is_empty() {
            true => 0,
            false => (self.right - self.left) as i64 * (self.bottom - self.top) as i64,
        }
    }

    fn from_rect(rect: &Rect, extent: Extent) -> PixelRect {
        let clamp =
            |value: f32, limit: u32| (value.floor().max(0.0) as i64).min(limit as i64) as i32;
        let clamp_up =
            |value: f32, limit: u32| (value.ceil().max(0.0) as i64).min(limit as i64) as i32;
        PixelRect {
            left: clamp(rect.x, extent.width),
            top: clamp(rect.y, extent.height),
            right: clamp_up(rect.x + rect.w, extent.width),
            bottom: clamp_up(rect.y + rect.h, extent.height),
        }
    }
}

const EMPTY: PixelRect = PixelRect {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

#[derive(Clone, Copy, Debug)]
pub struct RectSet {
    rects: [PixelRect; MAX_RECTS],
    len: usize,
}

impl PartialEq for RectSet {
    fn eq(&self, other: &RectSet) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for RectSet {}

impl RectSet {
    pub const fn empty() -> RectSet {
        RectSet {
            rects: [EMPTY; MAX_RECTS],
            len: 0,
        }
    }

    pub fn as_slice(&self) -> &[PixelRect] {
        &self.rects[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn single(rect: PixelRect) -> RectSet {
        let mut set = RectSet::empty();
        set.rects[0] = rect;
        set.len = 1;
        set
    }

    fn push(&mut self, rect: PixelRect) {
        self.rects[self.len] = rect;
        self.len += 1;
    }
}

fn normalized(rects: impl Iterator<Item = PixelRect>) -> Damage {
    let mut set = RectSet::empty();
    let mut collapsed: Option<PixelRect> = None;
    for rect in rects.filter(|rect| !rect.is_empty()) {
        match (
            collapsed,
            set.as_slice().contains(&rect),
            set.len == MAX_RECTS,
        ) {
            (Some(bounds), _, _) => collapsed = Some(bounds.union(rect)),
            (None, true, _) => {}
            (None, false, true) => {
                collapsed = Some(set.as_slice().iter().copied().fold(rect, PixelRect::union))
            }
            (None, false, false) => set.push(rect),
        }
    }
    Damage::Rects(collapsed.map_or(set, RectSet::single))
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    Full,
    Rects(RectSet),
}

impl Damage {
    pub fn none() -> Damage {
        Damage::Rects(RectSet::empty())
    }

    pub fn rects(rects: &[PixelRect]) -> Damage {
        normalized(rects.iter().copied())
    }

    pub fn from_rects(rects: &[Rect], extent: Extent) -> Damage {
        let whole = PixelRect {
            left: 0,
            top: 0,
            right: extent.width as i32,
            bottom: extent.height as i32,
        };
        let pixels = rects.iter().map(|rect| PixelRect::from_rect(rect, extent));
        match rects.is_empty() || pixels.clone().any(|pixel| pixel == whole) {
            true => Damage::Full,
            false => normalized(pixels),
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Damage::Rects(rects) if rects.is_empty())
    }

    pub fn merge(&self, other: &Damage) -> Damage {
        match (self, other) {
            (Damage::Full, _) | (_, Damage::Full) => Damage::Full,
            (Damage::Rects(left), Damage::Rects(right)) => {
                normalized(left.as_slice().iter().chain(right.as_slice()).copied())
            }
        }
    }

    pub fn present_rects(&self) -> &[PixelRect] {
        match self {
            Damage::Full => &[],
            Damage::Rects(rects) => rects.as_slice(),
        }
    }

    pub fn covered_area(&self, extent: Extent) -> i64 {
        match self {
            Damage::Full => extent.width as i64 * extent.height as i64,
            Damage::Rects(rects) => rects.as_slice().iter().map(PixelRect::area).sum(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DamageTracker {
    last_presented: Vec<Option<u64>>,
    history: VecDeque<(u64, Damage)>,
    frame: u64,
}

impl DamageTracker {
    pub fn new(buffers: usize) -> Self {
        Self {
            last_presented: vec![None; buffers],
            history: VecDeque::with_capacity(HISTORY + 1),
            frame: 0,
        }
    }

    pub fn reset(&mut self) {
        self.last_presented.iter_mut().for_each(|slot| *slot = None);
        self.history.clear();
    }

    pub fn has_presented(&self) -> bool {
        self.last_presented.iter().any(Option::is_some)
    }

    pub fn age(&self, buffer: usize) -> u32 {
        match self.last_presented.get(buffer).copied().flatten() {
            Some(frame) => (self.frame - frame + 1) as u32,
            None => 0,
        }
    }

    pub fn repaint(&self, buffer: usize, current: &Damage) -> Damage {
        match self.last_presented.get(buffer).copied().flatten() {
            None => Damage::Full,
            Some(last) => {
                let first_needed = last + 1;
                let covered = self
                    .history
                    .front()
                    .map(|(frame, _)| *frame <= first_needed)
                    .unwrap_or(first_needed > self.frame);
                match covered {
                    false => Damage::Full,
                    true => self
                        .history
                        .iter()
                        .filter(|(frame, _)| *frame > last)
                        .fold(*current, |accumulated, (_, damage)| {
                            accumulated.merge(damage)
                        }),
                }
            }
        }
    }

    pub fn commit(&mut self, buffer: usize, damage: Damage) {
        self.frame += 1;
        self.history.push_back((self.frame, damage));
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        if let Some(slot) = self.last_presented.get_mut(buffer) {
            *slot = Some(self.frame);
        }
    }
}
