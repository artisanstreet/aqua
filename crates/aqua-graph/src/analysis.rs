use std::collections::HashMap;

use aqua_core::geometry::Rect;

use crate::plan::{Pass, TextureRef, TransientId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lifetime {
    pub first: usize,
    pub last: usize,
}

impl Lifetime {
    pub fn overlaps(self, other: Lifetime) -> bool {
        self.first <= other.last && other.first <= self.last
    }
}

pub fn transient_of(reference: &TextureRef) -> Option<TransientId> {
    match reference {
        TextureRef::Transient(id) => Some(*id),
        TextureRef::Surface | TextureRef::Persistent(_) => None,
    }
}

pub fn lifetimes(passes: &[Pass]) -> HashMap<TransientId, Lifetime> {
    let mut result: HashMap<TransientId, Lifetime> = HashMap::new();
    for (index, pass) in passes.iter().enumerate() {
        pass.reads
            .iter()
            .chain(&pass.writes)
            .filter_map(transient_of)
            .for_each(|id| {
                result
                    .entry(id)
                    .and_modify(|lifetime| lifetime.last = index)
                    .or_insert(Lifetime {
                        first: index,
                        last: index,
                    });
            });
    }
    result
}

pub fn lifetime_of(passes: &[Pass], id: TransientId) -> Option<Lifetime> {
    let target = TextureRef::Transient(id);
    let touches = |pass: &Pass| pass.reads.contains(&target) || pass.writes.contains(&target);
    let first = passes.iter().position(touches)?;
    let last = passes.iter().rposition(touches)?;
    Some(Lifetime { first, last })
}

pub fn first_writers(passes: &[Pass]) -> HashMap<TextureRef, usize> {
    let mut result = HashMap::new();
    for (index, pass) in passes.iter().enumerate() {
        pass.writes.iter().for_each(|write| {
            result.entry(*write).or_insert(index);
        });
    }
    result
}

pub fn independent(a: &Pass, b: &Pass) -> bool {
    let conflict = |x: &Pass, y: &Pass| {
        x.writes
            .iter()
            .any(|write| y.reads.contains(write) || y.writes.contains(write))
    };
    !conflict(a, b) && !conflict(b, a)
}

pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    (right > left && bottom > top).then(|| Rect::from_edges(left, top, right, bottom))
}

pub fn union(a: Rect, b: Rect) -> Rect {
    Rect::from_edges(
        a.x.min(b.x),
        a.y.min(b.y),
        a.right().max(b.right()),
        a.bottom().max(b.bottom()),
    )
}

pub fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment.max(1)) * alignment.max(1)
}
