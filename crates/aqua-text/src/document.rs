use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SelectionSourceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourcePosition {
    pub source: SelectionSourceId,
    pub offset: usize,
}

#[derive(Clone, Debug)]
pub struct SelectionSource {
    pub id: SelectionSourceId,
    pub order: Vec<u64>,
    pub text: Arc<str>,
    pub separator: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSlice {
    pub source: SelectionSourceId,
    pub range: Range<usize>,
}

impl SourceSlice {
    pub fn position(&self, local_offset: usize) -> SourcePosition {
        SourcePosition {
            source: self.source,
            offset: self
                .range
                .start
                .saturating_add(local_offset)
                .min(self.range.end),
        }
    }

    pub fn selected_range(&self, document: &SelectionDocument) -> Option<Range<usize>> {
        let range = document.range_for(self.source)?;
        let start = range.start.max(self.range.start);
        let end = range.end.min(self.range.end);
        (start < end).then(|| start - self.range.start..end - self.range.start)
    }
}

#[derive(Default)]
pub struct SelectionDocument {
    sources: HashMap<SelectionSourceId, SelectionSource>,
    order: Vec<SelectionSourceId>,
    indices: HashMap<SelectionSourceId, usize>,
    selection: Option<(SourcePosition, SourcePosition)>,
}

impl SelectionDocument {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, source: SelectionSource) {
        let reorder = self
            .sources
            .get(&source.id)
            .is_none_or(|previous| previous.order != source.order);
        self.sources.insert(source.id, source);
        if reorder {
            self.rebuild_order();
        }
        self.normalize_selection();
    }

    pub fn register_many(&mut self, sources: impl IntoIterator<Item = SelectionSource>) {
        for source in sources {
            self.sources.insert(source.id, source);
        }
        self.rebuild_order();
        self.normalize_selection();
    }

    pub fn unregister(&mut self, id: SelectionSourceId) -> bool {
        let removed = self.sources.remove(&id).is_some();
        if removed {
            self.rebuild_order();
            self.normalize_selection();
        }
        removed
    }

    pub fn source(&self, id: SelectionSourceId) -> Option<&SelectionSource> {
        self.sources.get(&id)
    }

    pub fn clear(&mut self) {
        self.sources.clear();
        self.order.clear();
        self.indices.clear();
        self.selection = None;
    }

    pub fn compare_positions(
        &self,
        left: SourcePosition,
        right: SourcePosition,
    ) -> Option<std::cmp::Ordering> {
        let left = self.normalize_position(left)?;
        let right = self.normalize_position(right)?;
        Some(
            (self.indices[&left.source], left.offset)
                .cmp(&(self.indices[&right.source], right.offset)),
        )
    }

    pub fn validate_slice(&self, slice: &SourceSlice) -> bool {
        let Some(source) = self.sources.get(&slice.source) else {
            return false;
        };
        slice.range.start <= slice.range.end
            && slice.range.end <= source.text.len()
            && self
                .normalize_position(SourcePosition {
                    source: slice.source,
                    offset: slice.range.start,
                })
                .is_some_and(|position| position.offset == slice.range.start)
            && self
                .normalize_position(SourcePosition {
                    source: slice.source,
                    offset: slice.range.end,
                })
                .is_some_and(|position| position.offset == slice.range.end)
    }

    pub fn source_ids(&self) -> &[SelectionSourceId] {
        &self.order
    }

    pub fn selection(&self) -> Option<(SourcePosition, SourcePosition)> {
        self.selection
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn set_selection(&mut self, anchor: SourcePosition, focus: SourcePosition) -> bool {
        let Some(anchor) = self.normalize_position(anchor) else {
            return false;
        };
        let Some(focus) = self.normalize_position(focus) else {
            return false;
        };
        self.selection = Some((anchor, focus));
        true
    }

    pub fn select_all(&mut self) -> bool {
        let Some(first) = self.order.first().copied() else {
            return false;
        };
        let last = *self.order.last().unwrap();
        self.set_selection(
            SourcePosition {
                source: first,
                offset: 0,
            },
            SourcePosition {
                source: last,
                offset: self.sources[&last].text.len(),
            },
        )
    }

    pub fn range_for(&self, id: SelectionSourceId) -> Option<Range<usize>> {
        let (start, end) = self.ordered_selection()?;
        let index = *self.indices.get(&id)?;
        let first = self.indices[&start.source];
        let last = self.indices[&end.source];
        if index < first || index > last {
            return None;
        }
        let from = if id == start.source { start.offset } else { 0 };
        let to = if id == end.source {
            end.offset
        } else {
            self.sources.get(&id)?.text.len()
        };
        Some(from..to)
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.ordered_selection()?;
        if start == end {
            return None;
        }
        let first = self.indices[&start.source];
        let last = self.indices[&end.source];
        let mut copied = String::new();
        for (index, id) in self.order[first..=last].iter().enumerate() {
            let source = &self.sources[id];
            if index > 0 {
                copied.push_str(&source.separator);
            }
            let range = self.range_for(*id)?;
            copied.push_str(&source.text[range]);
        }
        (!copied.is_empty()).then_some(copied)
    }

    fn ordered_selection(&self) -> Option<(SourcePosition, SourcePosition)> {
        let (anchor, focus) = self.selection?;
        let anchor_key = (*self.indices.get(&anchor.source)?, anchor.offset);
        let focus_key = (*self.indices.get(&focus.source)?, focus.offset);
        Some(if anchor_key <= focus_key {
            (anchor, focus)
        } else {
            (focus, anchor)
        })
    }

    fn normalize_position(&self, position: SourcePosition) -> Option<SourcePosition> {
        let text = &self.sources.get(&position.source)?.text;
        let at = position.offset.min(text.len());
        let offset = if at == text.len() {
            at
        } else {
            text.grapheme_indices(true)
                .map(|(index, _)| index)
                .take_while(|index| *index <= at)
                .last()
                .unwrap_or(0)
        };
        Some(SourcePosition {
            source: position.source,
            offset,
        })
    }

    fn normalize_selection(&mut self) {
        self.selection = self.selection.and_then(|(anchor, focus)| {
            Some((
                self.normalize_position(anchor)?,
                self.normalize_position(focus)?,
            ))
        });
    }

    fn rebuild_order(&mut self) {
        self.order = self.sources.keys().copied().collect();
        self.order.sort_by(|a, b| {
            self.sources[a]
                .order
                .cmp(&self.sources[b].order)
                .then_with(|| a.cmp(b))
        });
        self.indices = self
            .order
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index))
            .collect();
    }
}
