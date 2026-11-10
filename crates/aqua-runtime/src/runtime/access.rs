use super::*;
use aqua_protocol::{A11yText, A11yTextRun};
use unicode_segmentation::UnicodeSegmentation;

#[derive(PartialEq)]
pub(super) struct AccessRunKey {
    revision: u64,
    source: String,
    origin: Point,
    transform: aqua_core::Transform2D,
    scale: f32,
}

pub(super) struct AccessRuns {
    key: AccessRunKey,
    runs: Vec<A11yTextRun>,
}

impl Runtime {
    pub(super) fn text_accessibility(&self, node: NodeId) -> Option<A11yText> {
        let NodeKind::Text(text) = &self.tree.get(node)?.kind else {
            return None;
        };
        if text.password {
            return None;
        }
        let entry = self.text_runs.get(&node)?;
        let registry = self.runs.lock();
        let handle = registry.resolve(entry.run)?;
        let layout = handle.layout;
        let source = match text.is_placeholder() {
            true => String::new(),
            false => text.display_text(),
        };
        let padding = self.tree.get(node)?.style.padding;
        let transform = self.tree.visual_transform(node)?;
        let origin = Point::new(padding.left - entry.scroll_x, padding.top);
        let selection = match &text.backing {
            TextBacking::Editable(editor) if !editor.is_composing() => {
                let selection = editor.selection();
                Some((selection.anchor, selection.caret))
            }
            TextBacking::Static(_) => text
                .selection
                .map(|selection| (selection.anchor, selection.caret)),
            _ => None,
        };
        let key = AccessRunKey {
            revision: handle.revision,
            source: source.clone(),
            origin,
            transform,
            scale: self.scale,
        };
        if let Some(cached) = entry
            .access_runs
            .borrow()
            .as_ref()
            .filter(|cached| cached.key == key)
        {
            return Some(A11yText {
                runs: cached.runs.clone(),
                selection,
                selection_enabled: text.is_editable() || text.selectable,
            });
        }
        let world_rect = |left: f32, top: f32, right: f32, bottom: f32| {
            let start = transform.apply(Point::new(
                origin.x + left / self.scale,
                origin.y + top / self.scale,
            ));
            let end = transform.apply(Point::new(
                origin.x + right / self.scale,
                origin.y + bottom / self.scale,
            ));
            Rect::from_edges(
                start.x * self.scale,
                start.y * self.scale,
                end.x * self.scale,
                end.y * self.scale,
            )
        };
        let mut units = Vec::new();
        for (offset, grapheme) in source.grapheme_indices(true) {
            match u8::try_from(grapheme.len()) {
                Ok(_) => units.push((offset, grapheme)),
                Err(_) => units.extend(grapheme.char_indices().map(|(index, ch)| {
                    (
                        offset + index,
                        &source[offset + index..offset + index + ch.len_utf8()],
                    )
                })),
            }
        }
        let boundaries = aqua_text::navigation::WordBoundaries::new(&source);
        let mut word_starts = HashSet::from([0]);
        let mut word = 0;
        while word < source.len() {
            let next = boundaries.next(
                &source,
                word,
                aqua_text::navigation::WordBehavior::platform(),
            );
            if next <= word {
                break;
            }
            word_starts.insert(next);
            word = next;
        }
        let mut runs = Vec::<A11yTextRun>::new();
        for (offset, unit) in units {
            let caret = layout.caret(offset)?;
            let next = layout.caret(offset + unit.len()).unwrap_or(caret);
            let rtl = !unit.ends_with('\n') && next.y_top == caret.y_top && next.x < caret.x;
            let rect = layout
                .selection_rects(offset..offset + unit.len())
                .first()
                .copied();
            let left = rect.map_or(caret.x, |rect| rect.x);
            let right = rect.map_or(caret.x, |rect| rect.x + rect.w);
            let bounds = world_rect(left, caret.y_top, right, caret.y_top + caret.height);
            let top = bounds.y;
            let new_run = runs.last().is_none_or(|run| {
                run.character_lengths.len() >= 255
                    || run.rtl != rtl
                    || (run.bounds.y - top).abs() > 0.01
                    || run.text.ends_with('\n')
            });
            if new_run {
                runs.push(A11yTextRun {
                    text: String::new(),
                    byte_start: offset,
                    bounds,
                    word_starts: Vec::new(),
                    character_lengths: Vec::new(),
                    character_positions: Vec::new(),
                    character_widths: Vec::new(),
                    rtl,
                });
            }
            let run = runs.last_mut()?;
            if word_starts.contains(&offset) {
                run.word_starts.push(run.character_lengths.len() as u8);
            }
            run.text.push_str(unit);
            run.character_lengths.push(unit.len() as u8);
            run.bounds = run.bounds.union(bounds);
            run.character_positions.push(bounds.x);
            run.character_widths.push(bounds.w);
        }
        if runs.is_empty() {
            let caret = layout.caret(0)?;
            runs.push(A11yTextRun {
                text: String::new(),
                byte_start: 0,
                bounds: world_rect(caret.x, caret.y_top, caret.x, caret.y_top + caret.height),
                word_starts: Vec::new(),
                character_lengths: Vec::new(),
                character_positions: Vec::new(),
                character_widths: Vec::new(),
                rtl: false,
            });
        }
        for run in &mut runs {
            for (position, width) in run
                .character_positions
                .iter_mut()
                .zip(&run.character_widths)
            {
                *position = match run.rtl {
                    true => run.bounds.right() - *position - width,
                    false => *position - run.bounds.x,
                };
            }
        }
        *entry.access_runs.borrow_mut() = Some(AccessRuns {
            key,
            runs: runs.clone(),
        });
        Some(A11yText {
            runs,
            selection,
            selection_enabled: text.is_editable() || text.selectable,
        })
    }

    pub(super) fn apply_access_byte_selection(
        &mut self,
        node: NodeId,
        focus_node: NodeId,
        anchor_byte: usize,
        focus_byte: usize,
    ) {
        if node != focus_node {
            return;
        }
        let allowed = self.tree.get(node).is_some_and(|record| matches!(&record.kind, NodeKind::Text(text) if !text.password && (text.is_editable() || text.selectable)));
        if !allowed {
            return;
        }
        self.selection_from_access(node, Selection::new(anchor_byte, focus_byte));
    }
}
