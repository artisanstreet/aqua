use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use aqua_media::hash::{FastHasher, FastMap};

use crate::shape::{ShapeContext, ShapedParagraph};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ParagraphKey(u64, u64);

const SALT: u64 = 0x9e37_79b9_7f4a_7c15;

pub(crate) fn paragraph_key(
    context: &ShapeContext<'_>,
    display: &str,
    range: &Range<usize>,
    fonts_revision: u64,
) -> ParagraphKey {
    let mut first = FastHasher::default();
    let mut second = FastHasher::default();
    second.write_u64(SALT);
    for hasher in [&mut first, &mut second] {
        feed(hasher, context, display, range, fonts_revision);
    }
    ParagraphKey(first.finish(), second.finish())
}

fn feed(
    hasher: &mut FastHasher,
    context: &ShapeContext<'_>,
    display: &str,
    range: &Range<usize>,
    fonts_revision: u64,
) {
    hasher.write_u64(fonts_revision);
    hasher.write_u32(context.scale.to_bits());
    hasher.write_u8(u8::from(context.gpui_line_height) | u8::from(context.inline_cells) << 1);
    hasher.write_usize(range.start);
    display[range.clone()].hash(hasher);
    for (index, span) in context.spans.iter().enumerate() {
        let span_range = &context.span_ranges[index].range;
        let from = span_range.start.max(range.start);
        let to = span_range.end.min(range.end);
        let first_empty_span = range.is_empty() && span_range.end > range.start;
        if from >= to && !first_empty_span {
            continue;
        }
        hasher.write_usize(index);
        hasher.write_usize(from.saturating_sub(range.start));
        hasher.write_usize(to.saturating_sub(range.start));
        span.style.shaping_hash(hasher);
        span.inline_cell_width().map(f32::to_bits).hash(hasher);
        if first_empty_span {
            break;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShapeCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub bytes: usize,
    pub rotations: u64,
}

pub const SHAPE_CACHE_BUDGET_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct ShapeCache {
    current: FastMap<ParagraphKey, Arc<ShapedParagraph>>,
    previous: FastMap<ParagraphKey, Arc<ShapedParagraph>>,
    current_bytes: usize,
    budget: usize,
    hits: u64,
    misses: u64,
    rotations: u64,
}

impl Default for ShapeCache {
    fn default() -> Self {
        Self::with_budget(SHAPE_CACHE_BUDGET_BYTES)
    }
}

pub(crate) fn paragraph_bytes(paragraph: &ShapedParagraph) -> usize {
    let runs: usize = paragraph
        .runs
        .iter()
        .map(|run| std::mem::size_of_val(run) + std::mem::size_of_val(run.glyphs.as_slice()))
        .sum();
    std::mem::size_of::<ShapedParagraph>()
        + std::mem::size_of_val(paragraph.clusters.as_slice())
        + runs
}

impl ShapeCache {
    pub(crate) fn with_budget(budget: usize) -> Self {
        Self {
            current: FastMap::default(),
            previous: FastMap::default(),
            current_bytes: 0,
            budget,
            hits: 0,
            misses: 0,
            rotations: 0,
        }
    }

    pub(crate) fn get(&mut self, key: &ParagraphKey) -> Option<Arc<ShapedParagraph>> {
        if let Some(found) = self.current.get(key) {
            self.hits += 1;
            return Some(Arc::clone(found));
        }
        let promoted = self.previous.remove(key)?;
        self.hits += 1;
        self.admit(*key, Arc::clone(&promoted));
        Some(promoted)
    }

    pub(crate) fn insert(&mut self, key: ParagraphKey, paragraph: Arc<ShapedParagraph>) {
        self.misses += 1;
        self.admit(key, paragraph);
    }

    fn admit(&mut self, key: ParagraphKey, paragraph: Arc<ShapedParagraph>) {
        let bytes = paragraph_bytes(&paragraph);
        if self.current_bytes + bytes > self.budget && !self.current.is_empty() {
            self.rotations += 1;
            self.previous = std::mem::take(&mut self.current);
            self.current_bytes = 0;
        }
        self.current_bytes += bytes;
        self.current.insert(key, paragraph);
    }

    pub(crate) fn clear(&mut self) {
        self.current = FastMap::default();
        self.previous = FastMap::default();
        self.current_bytes = 0;
    }

    pub(crate) fn stats(&self) -> ShapeCacheStats {
        ShapeCacheStats {
            hits: self.hits,
            misses: self.misses,
            entries: self.current.len() + self.previous.len(),
            bytes: self.current_bytes
                + self
                    .previous
                    .values()
                    .map(|paragraph| paragraph_bytes(paragraph))
                    .sum::<usize>(),
            rotations: self.rotations,
        }
    }
}
