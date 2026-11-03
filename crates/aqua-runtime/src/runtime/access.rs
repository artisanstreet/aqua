use super::*;
use aqua_protocol::{A11yText, A11yTextRun};
use unicode_segmentation::UnicodeSegmentation;

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
        let layout = registry.resolve(entry.run)?.layout;
        let source = match text.is_placeholder() {
            true => String::new(),
            false => text.display_text(),
        };
        let origin = self.text_origin(node)?;
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
            let top = origin.y * self.scale + caret.y_top;
            let bounds = Rect {
                x: origin.x * self.scale + left,
                y: top,
                w: (right - left).max(0.0),
                h: caret.height,
            };
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
                bounds: Rect {
                    x: origin.x * self.scale + caret.x,
                    y: origin.y * self.scale + caret.y_top,
                    w: 0.0,
                    h: caret.height,
                },
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
