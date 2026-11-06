use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, PointerButton, Rect,
};
use aqua_text::{
    document::{
        SelectionDocument, SelectionDocumentId, SelectionSource, SelectionSourceError,
        SelectionSourceId, SourcePosition, SourceSlice,
    },
    navigation::SelectionUnit,
    CaretAffinity, CaretPosition, Selection,
};
use aqua_ui::{
    collection_runtime::{CollectionRowRegistration, SelectionBatchOwner},
    events::hit_test,
    tree::{ActionEvent, ActionKind, NodeKind, TextBacking},
};

use super::Runtime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TextPosition {
    node: NodeId,
    byte: usize,
}

#[derive(Clone, Copy)]
struct DocumentSelection {
    anchor: TextPosition,
    focus: TextPosition,
}

struct DocumentIndex {
    root: NodeId,
    version: u64,
    nodes: Vec<NodeId>,
    positions: HashMap<NodeId, usize>,
}

impl DocumentIndex {
    fn compare(&self, left: TextPosition, right: TextPosition) -> Option<Ordering> {
        Some(
            self.positions
                .get(&left.node)?
                .cmp(self.positions.get(&right.node)?)
                .then_with(|| left.byte.cmp(&right.byte)),
        )
    }

    fn bounds(
        &self,
        selection: DocumentSelection,
    ) -> Option<(TextPosition, TextPosition, Range<usize>)> {
        let (start, end) = match self.compare(selection.anchor, selection.focus)? {
            Ordering::Greater => (selection.focus, selection.anchor),
            _ => (selection.anchor, selection.focus),
        };
        Some((
            start,
            end,
            *self.positions.get(&start.node)?..*self.positions.get(&end.node)? + 1,
        ))
    }
}

#[derive(Clone)]
struct Gesture {
    contact: u64,
    origin: Point,
    anchor: TextPosition,
    initial: Range<usize>,
    unit: SelectionUnit,
    editable: bool,
    external_text_input: bool,
    selection_auto_scroll: bool,
    dragged: bool,
    pointer: Point,
    advanced_at: f64,
    scroll: Option<NodeId>,
    root: NodeId,
    source_anchor: Option<SourcePosition>,
    source_anchor_end: Option<SourcePosition>,
}

#[derive(Clone, Copy)]
struct Click {
    node: NodeId,
    at: Point,
    time: f64,
    count: usize,
}

#[derive(Clone)]
struct RangePress {
    node: NodeId,
    range: aqua_text::TextInteractionRange,
    pointer: aqua_core::PointerEvent,
    dragged: bool,
}

#[derive(Default)]
pub(super) struct TextInteraction {
    click_settings: aqua_core::ClickSettings,
    modifiers: Modifiers,
    range_press: Option<RangePress>,
    gesture: Option<Gesture>,
    click: Option<Click>,
    selection: Option<DocumentSelection>,
    pub(super) selected_nodes: Vec<NodeId>,
    goal_x: Option<(NodeId, f32)>,
    external_vertical_revision: Option<(NodeId, u64)>,
    document: Option<Rc<DocumentIndex>>,
    source_document: SelectionDocument,
    source_document_id: SelectionDocumentId,
    source_documents: HashMap<SelectionDocumentId, SelectionDocument>,
    source_scopes: HashMap<NodeId, SelectionDocumentId>,
    descriptor_bindings: HashSet<NodeId>,
    binding_version: Option<u64>,
    source_slices: HashMap<NodeId, SourceSlice>,
    source_owner: Option<NodeId>,
    committed_batches: HashMap<(SelectionDocumentId, SelectionBatchOwner), CommittedBatch>,
}

struct CommittedBatch {
    identity: u64,
    revision: u64,
    sources: HashSet<SelectionSourceId>,
}

impl TextInteraction {
    pub(super) fn register_collection_sources(
        &mut self,
        registration: CollectionRowRegistration,
    ) -> Result<(), String> {
        let mut groups: HashMap<SelectionDocumentId, Vec<SelectionSource>> = HashMap::new();
        for (document, source) in registration.sources {
            groups.entry(document).or_default().push(source);
        }
        let mut pending = HashMap::new();
        let mut seen = HashMap::new();
        for batch in registration.batches {
            let key = (batch.document(), batch.owner());
            if seen
                .insert(key, batch.identity())
                .is_some_and(|identity| identity != batch.identity())
            {
                return Err("conflicting selection batches for one owner".into());
            }
            if self.committed_batches.get(&key).is_some_and(|existing| {
                existing.identity == batch.identity() && existing.revision == batch.revision()
            }) {
                continue;
            }
            if let Some(existing) = pending.get(&key) {
                let existing: &std::sync::Arc<aqua_ui::collection_runtime::SelectionSourceBatch> =
                    existing;
                if existing.identity() != batch.identity() {
                    return Err("conflicting selection batches for one owner".into());
                }
            } else {
                pending.insert(key, batch);
            }
        }
        let mut ownership = HashMap::new();
        for batch in pending.values() {
            for source in batch.sources() {
                if ownership
                    .insert((batch.document(), source.id), batch.owner())
                    .is_some_and(|owner| owner != batch.owner())
                {
                    return Err("selection source belongs to multiple batch owners".into());
                }
            }
        }
        for ((document, source), owner) in &ownership {
            if groups
                .get(document)
                .is_some_and(|sources| sources.iter().any(|item| item.id == *source))
                || self.committed_batches.iter().any(
                    |((existing_document, existing_owner), batch)| {
                        existing_document == document
                            && existing_owner != owner
                            && !pending.contains_key(&(*existing_document, *existing_owner))
                            && batch.sources.contains(source)
                    },
                )
            {
                return Err("selection source belongs to multiple batch owners".into());
            }
        }
        for batch in pending.values() {
            groups
                .entry(batch.document())
                .or_default()
                .extend(batch.sources().iter().cloned());
        }
        for sources in groups.values() {
            SelectionDocument::new()
                .register_many(sources.iter().cloned())
                .map_err(|error| error.to_string())?;
        }
        let invalidated: HashMap<_, HashSet<_>> = groups
            .iter()
            .map(|(document, sources)| {
                (*document, sources.iter().map(|source| source.id).collect())
            })
            .collect();
        for (document, sources) in groups {
            let target = if document == self.source_document_id {
                &mut self.source_document
            } else {
                self.source_documents.entry(document).or_default()
            };
            target
                .register_many(sources)
                .map_err(|error| error.to_string())?;
        }
        self.committed_batches.retain(|(document, _), batch| {
            invalidated
                .get(document)
                .is_none_or(|changed| batch.sources.is_disjoint(changed))
        });
        for (key, batch) in pending {
            self.committed_batches.insert(
                key,
                CommittedBatch {
                    identity: batch.identity(),
                    revision: batch.revision(),
                    sources: batch.sources().iter().map(|source| source.id).collect(),
                },
            );
        }
        self.binding_version = None;
        Ok(())
    }

    fn invalidate_batch_source(
        &mut self,
        document: SelectionDocumentId,
        source: SelectionSourceId,
    ) {
        self.committed_batches.retain(|(owner_document, _), batch| {
            *owner_document != document || !batch.sources.contains(&source)
        });
    }
}

impl Runtime {
    pub fn drag_snapshot(&self) -> Option<aqua_ui::input::DragSnapshot> {
        self.tree.drag_snapshot()
    }

    pub fn set_click_settings(&mut self, settings: aqua_core::ClickSettings) {
        self.tree.set_interaction_click_settings(settings);
        if self.interaction.click_settings != settings {
            self.interaction.click_settings = settings;
            self.interaction.click = None;
        }
    }

    pub(super) fn blur_text_interaction(&mut self) {
        self.cancel_text_interaction();
        self.interaction.modifiers = Modifiers::NONE;
        self.interaction.click = None;
    }

    pub(super) fn cancel_text_interaction(&mut self) {
        if let Some(gesture) = self.interaction.gesture.take() {
            self.tree.take_press(gesture.contact);
        }
        self.interaction.range_press = None;
    }

    pub(super) fn inline_cursor(&self) -> Option<aqua_core::CursorIcon> {
        if self.tree.input_is_captured()
            || self
                .interaction
                .gesture
                .as_ref()
                .is_some_and(|gesture| gesture.dragged)
        {
            return None;
        }
        let at = self.tree.pointer_position()?;
        let node = hit_test(&self.tree, at)?;
        self.inline_range_at(node, at)
            .map(|_| aqua_core::CursorIcon::Pointer)
    }

    fn inline_range_at(&self, node: NodeId, at: Point) -> Option<aqua_text::TextInteractionRange> {
        if !self.text_node_allowed(node) || self.tree.is_disabled(node) {
            return None;
        }
        let Some(NodeKind::Text(text)) = self.tree.get(node).map(|record| &record.kind) else {
            return None;
        };
        if text.password
            || matches!(&text.backing, TextBacking::Editable(editor) if editor.is_composing())
        {
            return None;
        }
        let point = self.text_local_point(node, at)?;
        let entry = self.text_runs.get(&node)?;
        let guard = self.runs.lock();
        let layout = guard.resolve(entry.run)?.layout;
        let byte = layout.word_hit(point.x, point.y)?;
        let spans = layout.interaction_ranges();
        text.interaction_ranges
            .iter()
            .chain(spans.iter())
            .find(|range| {
                range.range.start <= byte
                    && byte < range.range.end
                    && range.range.end <= text.text().len()
            })
            .cloned()
    }

    pub(super) fn handle_inline_input(&mut self, event: &InputEvent) -> bool {
        match event {
            InputEvent::PointerDown(pointer, PointerButton::Primary) => {
                self.interaction.range_press =
                    hit_test(&self.tree, pointer.position).and_then(|node| {
                        let repeated = self.interaction.click.is_some_and(|click| {
                            click.node == node
                                && self.clock - click.time
                                    <= self.interaction.click_settings.interval().as_secs_f64()
                                && (click.at.x - pointer.position.x).abs()
                                    <= self.interaction.click_settings.max_distance().x
                                && (click.at.y - pointer.position.y).abs()
                                    <= self.interaction.click_settings.max_distance().y
                        });
                        if repeated {
                            return None;
                        }
                        self.inline_range_at(node, pointer.position)
                            .map(|range| RangePress {
                                node,
                                range,
                                pointer: *pointer,
                                dragged: false,
                            })
                    });
            }
            InputEvent::PointerMove(pointer) => {
                if let Some(press) = &mut self.interaction.range_press {
                    if press.pointer.contact == pointer.contact {
                        press.dragged |= (press.pointer.position.x - pointer.position.x)
                            .abs()
                            .max((press.pointer.position.y - pointer.position.y).abs())
                            >= 3.0;
                    }
                }
            }
            InputEvent::PointerUp(pointer, PointerButton::Primary) => {
                let Some(press) = self
                    .interaction
                    .range_press
                    .take()
                    .filter(|press| press.pointer.contact == pointer.contact)
                else {
                    return false;
                };
                if press.dragged
                    || hit_test(&self.tree, pointer.position) != Some(press.node)
                    || self.inline_range_at(press.node, pointer.position).as_ref()
                        != Some(&press.range)
                {
                    return false;
                }
                self.cancel_text_interaction();
                self.tree.take_press(pointer.contact);
                let host = &self.host;
                aqua_ui::input::route_text_range(
                    &mut self.tree,
                    press.node,
                    press.range.id,
                    press.range.range,
                    *pointer,
                    |tree, node| host.input_allowed(tree, node),
                );
                return true;
            }
            InputEvent::Scroll { .. }
            | InputEvent::PointerCancel { .. }
            | InputEvent::Focused(false) => self.interaction.range_press = None,
            _ => {}
        }
        false
    }

    pub fn selected_text(&self) -> Option<String> {
        if let Some(node) = self.tree.focused() {
            if self.text_node_allowed(node) && !self.tree.is_disabled(node) {
                if let Some(NodeKind::Text(text)) = self.tree.get(node).map(|node| &node.kind) {
                    if text.password {
                        return None;
                    }
                    if let TextBacking::Editable(editor) = &text.backing {
                        return editor.selected_text().map(str::to_owned);
                    }
                }
            }
        }
        if self.interaction.source_document.selection().is_some() {
            return self
                .source_selection_allowed()
                .then(|| self.interaction.source_document.selected_text())
                .flatten();
        }
        let selection = self.interaction.selection?;
        let index = self.make_document_index(selection.anchor.node)?;
        let (_, _, bounds) = index.bounds(selection)?;
        let mut chunks = Vec::new();
        for node in index.nodes[bounds].iter().copied() {
            if self.interaction.source_slices.contains_key(&node) {
                continue;
            }
            let Some(NodeKind::Text(text)) = self.tree.get(node).map(|node| &node.kind) else {
                continue;
            };
            if text.password || !self.text_node_allowed(node) {
                continue;
            }
            if let Some(range) = self.document_range(&index, selection, node) {
                if let Some(chunk) = text.text().get(range) {
                    chunks.push(chunk);
                }
            }
        }
        let text = chunks.join("\n");
        (!text.is_empty()).then_some(text)
    }

    fn source_selection_allowed(&self) -> bool {
        let Some(owner) = self.interaction.source_owner else {
            return false;
        };
        if self.tree.get(owner).is_none()
            || self.tree.is_disabled(owner)
            || !self.text_node_allowed(owner)
        {
            return false;
        }
        if self.tree.focused().is_some_and(|focused| {
            self.tree.is_disabled(focused)
                || !self.text_node_allowed(focused)
                || self.document_root(focused) != Some(owner)
        }) {
            return false;
        }
        for (node, slice) in &self.interaction.source_slices {
            if self
                .interaction
                .source_scopes
                .get(node)
                .copied()
                .unwrap_or_default()
                != self.interaction.source_document_id
            {
                continue;
            }
            if self
                .interaction
                .source_document
                .range_for(slice.source)
                .is_none()
                || self.tree.get(*node).is_none()
            {
                continue;
            }
            if !self.selectable(*node)
                || !self.text_node_allowed(*node)
                || self.document_root(*node) != Some(owner)
                || !self.interaction.source_document.validate_slice(slice)
            {
                return false;
            }
            let Some(source) = self.interaction.source_document.source(slice.source) else {
                return false;
            };
            if !self.tree.get(*node).is_some_and(|record| matches!(&record.kind, NodeKind::Text(text) if !text.password && !text.is_editable() && source.text.get(slice.range.clone()) == Some(text.text()))) { return false; }
        }
        true
    }

    fn selection_document(&self, document: SelectionDocumentId) -> Option<&SelectionDocument> {
        if document == self.interaction.source_document_id {
            Some(&self.interaction.source_document)
        } else {
            self.interaction.source_documents.get(&document)
        }
    }

    fn selection_document_mut(&mut self, document: SelectionDocumentId) -> &mut SelectionDocument {
        if document == self.interaction.source_document_id {
            &mut self.interaction.source_document
        } else {
            self.interaction
                .source_documents
                .entry(document)
                .or_default()
        }
    }

    fn activate_selection_document(&mut self, document: SelectionDocumentId) {
        if self.interaction.source_document_id != document {
            self.clear_document_selection();
            let previous = std::mem::take(&mut self.interaction.source_document);
            self.interaction
                .source_documents
                .insert(self.interaction.source_document_id, previous);
            self.interaction.source_document = self
                .interaction
                .source_documents
                .remove(&document)
                .unwrap_or_default();
            self.interaction.source_document_id = document;
        }
    }

    pub fn register_selection_source(
        &mut self,
        source: SelectionSource,
    ) -> Result<(), SelectionSourceError> {
        self.register_selection_source_in(SelectionDocumentId::default(), source)
    }

    pub fn register_selection_source_in(
        &mut self,
        document: SelectionDocumentId,
        source: SelectionSource,
    ) -> Result<(), SelectionSourceError> {
        let id = source.id;
        self.selection_document_mut(document).register(source)?;
        self.interaction.invalidate_batch_source(document, id);
        self.interaction.binding_version = None;
        self.paint_source_selection();
        Ok(())
    }

    pub fn register_selection_sources(
        &mut self,
        sources: impl IntoIterator<Item = SelectionSource>,
    ) -> Result<(), SelectionSourceError> {
        self.register_selection_sources_in(SelectionDocumentId::default(), sources)
    }

    pub fn register_selection_sources_in(
        &mut self,
        document: SelectionDocumentId,
        sources: impl IntoIterator<Item = SelectionSource>,
    ) -> Result<(), SelectionSourceError> {
        let sources: Vec<_> = sources.into_iter().collect();
        let ids: Vec<_> = sources.iter().map(|source| source.id).collect();
        self.selection_document_mut(document)
            .register_many(sources)?;
        for id in ids {
            self.interaction.invalidate_batch_source(document, id);
        }
        self.interaction.binding_version = None;
        self.paint_source_selection();
        Ok(())
    }

    pub fn unregister_selection_source(&mut self, source: SelectionSourceId) -> bool {
        self.unregister_selection_source_in(SelectionDocumentId::default(), source)
    }

    pub fn unregister_selection_source_in(
        &mut self,
        document: SelectionDocumentId,
        source: SelectionSourceId,
    ) -> bool {
        let removed = self.selection_document_mut(document).unregister(source);
        self.interaction.invalidate_batch_source(document, source);
        self.interaction.source_slices.retain(|node, slice| {
            slice.source != source
                || self
                    .interaction
                    .source_scopes
                    .get(node)
                    .copied()
                    .unwrap_or_default()
                    != document
        });
        self.interaction.binding_version = None;
        self.paint_source_selection();
        removed
    }

    pub fn release_selection_batch_owner(
        &mut self,
        document: SelectionDocumentId,
        owner: SelectionBatchOwner,
    ) {
        self.interaction
            .committed_batches
            .remove(&(document, owner));
    }

    fn valid_source_binding(
        &self,
        document: SelectionDocumentId,
        node: NodeId,
        slice: &SourceSlice,
    ) -> bool {
        let Some(document) = self.selection_document(document) else {
            return false;
        };
        let Some(NodeKind::Text(text)) = self.tree.get(node).map(|record| &record.kind) else {
            return false;
        };
        let Some(source) = document.source(slice.source) else {
            return false;
        };
        document.validate_slice(slice)
            && self.selectable(node)
            && !text.is_editable()
            && !text.password
            && source.text.get(slice.range.clone()) == Some(text.text())
    }

    pub fn bind_selection_source(&mut self, node: NodeId, slice: SourceSlice) -> bool {
        self.bind_selection_source_in(SelectionDocumentId::default(), node, slice)
    }

    pub fn bind_selection_source_in(
        &mut self,
        document: SelectionDocumentId,
        node: NodeId,
        slice: SourceSlice,
    ) -> bool {
        if !self.valid_source_binding(document, node, &slice) || !self.text_node_allowed(node) {
            return false;
        }
        self.interaction.source_slices.insert(node, slice);
        self.interaction.source_scopes.insert(node, document);
        self.paint_source_selection();
        true
    }

    pub fn unbind_selection_source(&mut self, node: NodeId) {
        self.interaction.source_slices.remove(&node);
        self.interaction.source_scopes.remove(&node);
        self.interaction.descriptor_bindings.remove(&node);
        self.tree.set_text_selection(node, None);
        self.interaction
            .selected_nodes
            .retain(|selected| *selected != node);
    }

    pub fn clear_selection_sources(&mut self) {
        self.clear_document_selection();
        self.interaction.source_document.clear();
        self.interaction.source_documents.clear();
        self.interaction.committed_batches.clear();
        self.interaction.source_slices.clear();
        self.interaction.source_scopes.clear();
        self.interaction.descriptor_bindings.clear();
        self.interaction.binding_version = None;
    }

    pub(super) fn sync_selection_bindings(&mut self) {
        if self.interaction.binding_version == Some(self.tree.content_version()) {
            return;
        }
        for node in std::mem::take(&mut self.interaction.descriptor_bindings) {
            self.interaction.source_slices.remove(&node);
            self.interaction.source_scopes.remove(&node);
        }
        for node in self.tree.traverse() {
            let binding = self.tree.get(node).and_then(|record| match &record.kind {
                NodeKind::Text(text) => text.selection_source.clone(),
                _ => None,
            });
            if let Some(binding) = binding {
                if self.valid_source_binding(binding.document, node, &binding.slice) {
                    self.interaction.source_slices.insert(node, binding.slice);
                    self.interaction
                        .source_scopes
                        .insert(node, binding.document);
                    self.interaction.descriptor_bindings.insert(node);
                }
            }
        }
        self.interaction.binding_version = Some(self.tree.content_version());
        self.paint_source_selection();
    }

    pub(super) fn selection_from_access(&mut self, node: NodeId, selection: Selection) -> bool {
        if !self.selectable(node) || !self.text_node_allowed(node) {
            return false;
        }
        let Some(NodeKind::Text(text)) = self.tree.get(node).map(|record| &record.kind) else {
            return false;
        };
        if text.is_editable() {
            self.clear_document_selection();
            self.tree.set_focused(Some(node));
            return self.tree.set_text_selection(node, Some(selection));
        }
        if text.selection_source.is_some() && !self.interaction.source_slices.contains_key(&node) {
            return false;
        }
        if let Some(document) = self.interaction.source_scopes.get(&node).copied() {
            self.activate_selection_document(document);
        }
        self.set_document_selection(DocumentSelection {
            anchor: TextPosition {
                node,
                byte: selection.anchor,
            },
            focus: TextPosition {
                node,
                byte: selection.caret,
            },
        });
        true
    }

    fn source_position(&self, position: TextPosition) -> Option<SourcePosition> {
        if self
            .interaction
            .source_scopes
            .get(&position.node)
            .copied()
            .unwrap_or_default()
            != self.interaction.source_document_id
        {
            return None;
        }
        let slice = self.interaction.source_slices.get(&position.node)?;
        self.interaction
            .source_document
            .validate_slice(slice)
            .then(|| slice.position(position.byte))
    }

    fn paint_source_selection(&mut self) {
        self.interaction
            .source_slices
            .retain(|node, _| self.tree.get(*node).is_some());
        self.interaction
            .source_scopes
            .retain(|node, _| self.tree.get(*node).is_some());
        self.interaction
            .descriptor_bindings
            .retain(|node| self.tree.get(*node).is_some());
        if self.interaction.source_document.selection().is_none()
            && self.interaction.source_owner.is_none()
        {
            return;
        }
        for node in std::mem::take(&mut self.interaction.selected_nodes) {
            self.tree.set_text_selection(node, None);
        }
        if self.interaction.source_document.selection().is_none()
            || !self.source_selection_allowed()
        {
            return;
        }
        for (&node, slice) in &self.interaction.source_slices {
            if self
                .interaction
                .source_scopes
                .get(&node)
                .copied()
                .unwrap_or_default()
                != self.interaction.source_document_id
            {
                continue;
            }
            if !self.text_node_allowed(node) || !self.selectable(node) {
                continue;
            }
            let Some(source) = self.interaction.source_document.source(slice.source) else {
                continue;
            };
            let valid = self.interaction.source_document.validate_slice(slice) && self.tree.get(node).is_some_and(|record| {
                matches!(&record.kind, NodeKind::Text(text) if !text.password && !text.is_editable() && source.text.get(slice.range.clone()) == Some(text.text()))
            });
            if valid {
                if let Some(range) = slice.selected_range(&self.interaction.source_document) {
                    self.tree
                        .set_text_selection(node, Some(Selection::new(range.start, range.end)));
                    self.interaction.selected_nodes.push(node);
                }
            }
        }
    }

    pub fn delete_selected_text(&mut self) -> bool {
        let Some(node) = self.editable_target() else {
            return false;
        };
        let deleted = self
            .tree
            .edit_text(node, |text| match &mut text.backing {
                TextBacking::Editable(editor) => {
                    let selected = !editor.selection().is_caret();
                    editor.delete_selection();
                    selected
                }
                TextBacking::Static(_) => false,
            })
            .unwrap_or(false);
        if deleted {
            self.fire_text_changed(node);
        }
        deleted
    }

    pub(super) fn follow_external_text_carets(&mut self) {
        let queued = self.tree.take_follow_caret_nodes();
        if queued.is_empty() {
            return;
        }
        let mut pending = Vec::new();
        for node in queued {
            let Some(record) = self.tree.get(node) else {
                continue;
            };
            let NodeKind::Text(text) = &record.kind else {
                continue;
            };
            if !text.external_text_input || !text.follow_text_caret {
                continue;
            }
            let ready = text.viewport_scroll_max.is_some();
            let TextBacking::Editable(editor) = &text.backing else {
                continue;
            };
            let caret = editor.selection().caret;
            let mut scroll = None;
            let mut parent = record.parent;
            while let Some(ancestor) = parent.and_then(|id| self.tree.get(id).map(|a| (id, a))) {
                if matches!(ancestor.1.kind, NodeKind::Scroll(_)) {
                    scroll = Some(ancestor.0);
                    break;
                }
                parent = ancestor.1.parent;
            }
            match (ready, scroll) {
                (true, Some(scroll)) => pending.push((node, scroll, caret)),
                _ => self.tree.requeue_follow_caret(node),
            }
        }
        for (node, scroll, byte) in pending {
            if let Some(record) = self.tree.get_mut(node) {
                if let NodeKind::Text(text) = &mut record.kind {
                    text.follow_text_caret = false;
                }
            }
            let Some(caret) = self.text_position_rect(node, byte) else {
                continue;
            };
            let Some(viewport) = self.tree.visual_rect(scroll) else {
                continue;
            };
            let Some(mut offset) = self.tree.scroll_offset(scroll) else {
                continue;
            };
            let top = viewport.y + 8.0;
            let bottom = viewport.bottom() - 8.0;
            if caret.y < top {
                offset.y -= top - caret.y;
            } else if caret.bottom() > bottom {
                offset.y += caret.bottom() - bottom;
            } else {
                continue;
            }
            self.tree.scroll_to(scroll, offset);
        }
    }

    pub fn text_position_rect(&mut self, node: NodeId, byte: usize) -> Option<Rect> {
        let record = self.tree.get(node)?;
        let NodeKind::Text(text) = &record.kind else {
            return None;
        };
        if text.password
            || byte > text.display_text().len()
            || !text.display_text().is_char_boundary(byte)
        {
            return None;
        }
        self.sync_text_runs_for(Some(node));
        let transform = self.tree.visual_transform(node)?;
        let record = self.tree.get(node)?;
        let entry = self.text_runs.get(&node)?;
        let guard = self.runs.lock();
        let handle = guard.resolve(entry.run)?;
        let caret = handle.layout.caret_position(CaretPosition {
            byte,
            affinity: CaretAffinity::Downstream,
        })?;
        let padding = record.style.padding;
        let start = transform.apply(Point::new(
            padding.left - entry.scroll_x + caret.x / self.scale,
            padding.top + caret.y_top / self.scale,
        ));
        let end = transform.apply(Point::new(
            padding.left - entry.scroll_x + caret.x / self.scale,
            padding.top + (caret.y_top + caret.height) / self.scale,
        ));
        Some(Rect::from_edges(start.x, start.y, end.x, end.y))
    }

    pub fn text_range_rects(&mut self, node: NodeId, range: Range<usize>) -> Vec<Rect> {
        let valid = self.tree.get(node).is_some_and(|record| matches!(&record.kind,
            NodeKind::Text(text) if !text.password && range.start <= range.end && range.end <= text.text().len()
                && text.text().is_char_boundary(range.start) && text.text().is_char_boundary(range.end)
                && !matches!(&text.backing, TextBacking::Editable(editor) if editor.is_composing())));
        if !valid {
            return Vec::new();
        }
        self.sync_text_runs_for(Some(node));
        let Some(transform) = self.tree.visual_transform(node) else {
            return Vec::new();
        };
        let Some(record) = self.tree.get(node) else {
            return Vec::new();
        };
        let Some(entry) = self.text_runs.get(&node) else {
            return Vec::new();
        };
        let guard = self.runs.lock();
        let Some(run) = guard.resolve(entry.run) else {
            return Vec::new();
        };
        let padding = record.style.padding;
        run.layout
            .selection_rects(range)
            .into_iter()
            .map(|rect| {
                let start = transform.apply(Point::new(
                    padding.left - entry.scroll_x + rect.x / self.scale,
                    padding.top + rect.y / self.scale,
                ));
                let end = transform.apply(Point::new(
                    padding.left - entry.scroll_x + (rect.x + rect.w) / self.scale,
                    padding.top + (rect.y + rect.h) / self.scale,
                ));
                Rect::from_edges(start.x, start.y, end.x, end.y)
            })
            .collect()
    }

    pub fn cut_external_selection(&mut self) -> Option<bool> {
        let node = self.tree.focused()?;
        let external = self.tree.get(node).is_some_and(|record| {
            matches!(
                &record.kind, NodeKind::Text(text) if text.external_text_input
            )
        });
        if !external {
            return None;
        }
        let host = &self.host;
        let outcome = aqua_ui::input::route_clipboard_cut(&mut self.tree, node, |tree, target| {
            host.input_allowed(tree, target)
        });
        self.tree.drain_commands();
        Some(outcome.handled || outcome.default_prevented)
    }

    pub fn paste_clipboard(&mut self, content: aqua_ui::input::ClipboardPaste) -> bool {
        let external = self
            .tree
            .focused()
            .and_then(|node| self.tree.get(node))
            .is_some_and(
                |record| matches!(&record.kind, NodeKind::Text(text) if text.external_text_input),
            );
        if aqua_ui::events::text_is_composing(&self.tree) && !external {
            return false;
        }
        let text = content.text.clone();
        let host = &self.host;
        let routed = aqua_ui::input::route_clipboard(&mut self.tree, content, |tree, node| {
            host.input_allowed(tree, node)
        });
        let pasted = if routed.default_prevented {
            false
        } else {
            text.as_ref().is_some_and(|text| self.paste_text(text))
        };
        self.tree.drain_commands();
        self.process_control_requests();
        pasted || routed.handled || routed.default_prevented
    }

    pub fn paste_text(&mut self, text: &str) -> bool {
        let Some(node) = self.editable_target() else {
            return false;
        };
        if aqua_ui::events::text_is_composing(&self.tree) {
            return false;
        }
        self.interaction.goal_x = None;
        let normalized: String = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .collect();
        if normalized.is_empty() {
            return false;
        }
        self.tree.edit_text(node, |text| {
            if let TextBacking::Editable(editor) = &mut text.backing {
                editor.insert_paste(&normalized);
            }
        });
        self.fire_text_changed(node);
        true
    }

    fn editable_target(&self) -> Option<NodeId> {
        let node = self.tree.focused()?;
        let enabled = self.text_node_allowed(node) && !self.tree.is_disabled(node);
        (enabled
            && self.tree.get(node).is_some_and(
                |record| matches!(&record.kind, NodeKind::Text(text) if text.is_editable()),
            ))
        .then_some(node)
    }

    fn fire_text_changed(&mut self, node: NodeId) {
        let text = self.tree.get(node).and_then(|record| match &record.kind {
            NodeKind::Text(text) => Some(text.text().to_owned()),
            _ => None,
        });
        self.tree.fire_action(ActionEvent {
            source: None,
            node,
            kind: ActionKind::TextChanged,
            text,
            focused: None,
        });
    }

    pub(super) fn handle_text_interaction(&mut self, event: &InputEvent) -> bool {
        if self
            .interaction
            .gesture
            .as_ref()
            .is_some_and(|gesture| !self.gesture_allowed(gesture))
        {
            if let Some(gesture) = self.interaction.gesture.take() {
                self.tree.take_press(gesture.contact);
            }
        }
        match event {
            InputEvent::PointerDown(pointer, PointerButton::Secondary) => {
                let Some(node) = hit_test(&self.tree, pointer.position) else {
                    return false;
                };
                if !self.text_node_allowed(node)
                    || !self
                        .tree
                        .get(node)
                        .is_some_and(|record| matches!(&record.kind, NodeKind::Text(_)))
                {
                    return false;
                }
                let Some(position) = self.hit_text_position(node, pointer.position) else {
                    return false;
                };
                let host = &self.host;
                let result = aqua_ui::input::route_text_context(
                    &mut self.tree,
                    node,
                    *pointer,
                    position.byte,
                    |tree, target| host.input_allowed(tree, target),
                );
                self.tree.drain_commands();
                result.handled || result.default_prevented
            }
            InputEvent::ModifiersChanged(modifiers) => {
                self.interaction.modifiers = *modifiers;
                false
            }
            InputEvent::KeyDown(key) => {
                self.interaction.modifiers = key.modifiers;
                self.interaction.gesture = None;
                let external = self.tree.focused().and_then(|node| self.tree.get(node)).is_some_and(
                    |record| matches!(&record.kind, NodeKind::Text(text) if text.external_text_input),
                );
                if !external
                    && aqua_ui::events::text_is_composing(&self.tree)
                    && self.text_input_allowed()
                {
                    return true;
                }
                if self.navigate_text(key) {
                    return true;
                }
                self.interaction.goal_x = None;
                if key.modifiers.contains(Modifiers::CONTROL)
                    && !key.modifiers.contains(Modifiers::ALT)
                    && !key.modifiers.contains(Modifiers::LOGO)
                    && key.key == Key::Char('a')
                    && self.editable_target().is_none()
                {
                    return self.select_document();
                }
                false
            }
            InputEvent::TextInput { .. }
            | InputEvent::Ime(_)
            | InputEvent::ImeReplace { .. }
            | InputEvent::ImeSelect { .. } => {
                self.interaction.goal_x = None;
                false
            }
            InputEvent::PointerMove(pointer) => {
                let Some(mut gesture) = self.interaction.gesture.clone() else {
                    return false;
                };
                if gesture.contact != pointer.contact {
                    return false;
                }
                let distance = (pointer.position.x - gesture.origin.x)
                    .abs()
                    .max((pointer.position.y - gesture.origin.y).abs());
                gesture.dragged |= distance >= 3.0;
                gesture.pointer = pointer.position;
                gesture.advanced_at = self.clock;
                if gesture.external_text_input {
                    self.route_external_pointer(
                        gesture.anchor.node,
                        *pointer,
                        aqua_ui::input::TextPointerPhase::Drag,
                        gesture.unit,
                    );
                    self.interaction.gesture = Some(gesture);
                    return true;
                }
                if gesture.dragged {
                    self.tree.take_press(pointer.contact);
                    self.extend_pointer_selection(&gesture, pointer.position);
                }
                self.interaction.gesture = Some(gesture);
                true
            }
            InputEvent::PointerUp(pointer, PointerButton::Primary) => {
                let Some(gesture) = self
                    .interaction
                    .gesture
                    .clone()
                    .filter(|gesture| gesture.contact == pointer.contact)
                else {
                    return false;
                };
                self.interaction.gesture = None;
                if gesture.external_text_input {
                    self.route_external_pointer(
                        gesture.anchor.node,
                        *pointer,
                        aqua_ui::input::TextPointerPhase::Up,
                        gesture.unit,
                    );
                    return true;
                }
                if gesture.dragged {
                    self.tree.take_press(pointer.contact);
                    self.extend_pointer_selection(&gesture, pointer.position);
                }
                gesture.dragged
            }
            InputEvent::PointerCancel { contact } => {
                if self
                    .interaction
                    .gesture
                    .as_ref()
                    .is_some_and(|gesture| gesture.contact == *contact)
                {
                    self.interaction.gesture = None;
                }
                false
            }
            InputEvent::Focused(false) => {
                self.interaction.gesture = None;
                self.interaction.click = None;
                self.interaction.modifiers = Modifiers::NONE;
                false
            }
            _ => false,
        }
    }

    pub(super) fn begin_text_interaction(&mut self, event: &InputEvent) -> bool {
        let InputEvent::PointerDown(pointer, PointerButton::Primary) = event else {
            return false;
        };
        self.interaction.goal_x = None;
        let hit = hit_test(&self.tree, pointer.position);
        let Some(node) = hit.filter(|node| self.selectable(*node)) else {
            self.clear_document_selection();
            self.interaction.click = None;
            return false;
        };
        if !self.text_node_allowed(node) {
            return false;
        }
        let declared = self.tree.get(node).is_some_and(|record| matches!(&record.kind, NodeKind::Text(text) if text.selection_source.is_some()));
        if declared && !self.interaction.source_slices.contains_key(&node) {
            self.clear_document_selection();
            return false;
        }
        if let Some(document) = self.interaction.source_scopes.get(&node).copied() {
            self.activate_selection_document(document);
        } else if self.interaction.source_document.selection().is_some() {
            self.clear_document_selection();
        }
        let Some(position) = self.hit_text_position(node, pointer.position) else {
            return false;
        };
        let byte = position.byte;
        let count = self
            .interaction
            .click
            .filter(|click| {
                click.node == node
                    && self.clock - click.time
                        <= self.interaction.click_settings.interval().as_secs_f64()
                    && (click.at.x - pointer.position.x).abs()
                        <= self.interaction.click_settings.max_distance().x
                    && (click.at.y - pointer.position.y).abs()
                        <= self.interaction.click_settings.max_distance().y
            })
            .map_or(1, |click| click.count.saturating_add(1));
        self.interaction.click = Some(Click {
            node,
            at: pointer.position,
            time: self.clock,
            count,
        });
        let unit = SelectionUnit::from_click_count(count);
        let byte = if unit == SelectionUnit::Word {
            self.hit_text_word(node, pointer.position).unwrap_or(byte)
        } else {
            byte
        };
        let NodeKind::Text(text) = &self.tree.get(node).unwrap().kind else {
            return false;
        };
        let editable = text.is_editable();
        let external_text_input = text.external_text_input;
        let initial = unit.range_at(text.text(), byte);
        let shift = self.interaction.modifiers.contains(Modifiers::SHIFT);
        let anchor_byte = match &text.backing {
            TextBacking::Editable(editor) if shift => editor.selection().anchor,
            _ => initial.start,
        };
        let anchor = if !editable && shift {
            self.interaction
                .selection
                .filter(|selection| self.same_document(selection.anchor.node, node))
                .map_or(
                    TextPosition {
                        node,
                        byte: anchor_byte,
                    },
                    |selection| selection.anchor,
                )
        } else {
            TextPosition {
                node,
                byte: anchor_byte,
            }
        };
        let focus = TextPosition {
            node,
            byte: initial.end,
        };
        let previous_source_anchor = (shift
            && self.interaction.source_owner == self.document_root(node))
        .then(|| {
            self.interaction
                .source_document
                .selection()
                .map(|(anchor, _)| anchor)
        })
        .flatten();
        let source_anchor = previous_source_anchor.or_else(|| self.source_position(anchor));
        let source_anchor_end = self.source_position(TextPosition {
            node: anchor.node,
            byte: initial.end,
        });
        if editable && !external_text_input {
            self.clear_document_selection();
            self.tree.set_focused(Some(node));
            self.tree
                .set_text_selection(node, Some(Selection::new(anchor.byte, focus.byte)));
            let target = CaretPosition {
                byte: focus.byte,
                affinity: if unit == SelectionUnit::Character {
                    position.affinity
                } else {
                    CaretAffinity::Upstream
                },
            };
            self.tree.edit_text(node, |text| {
                if let TextBacking::Editable(editor) = &mut text.backing {
                    editor.move_caret_position(target, true);
                }
            });
        } else if !editable {
            self.tree.set_focused(None);
            self.set_document_selection(DocumentSelection { anchor, focus });
            if let (Some(anchor), Some(focus)) =
                (previous_source_anchor, self.source_position(focus))
            {
                self.interaction
                    .source_document
                    .set_selection(anchor, focus);
                self.paint_source_selection();
            }
        }
        self.interaction.gesture = Some(Gesture {
            contact: pointer.contact,
            origin: pointer.position,
            anchor,
            initial,
            unit,
            editable,
            external_text_input,
            selection_auto_scroll: self.tree.get(node).is_none_or(
                |record| !matches!(&record.kind, NodeKind::Text(text) if !text.selection_auto_scroll),
            ),
            dragged: false,
            pointer: pointer.position,
            advanced_at: self.clock,
            scroll: self
                .document_root(node)
                .filter(|root| self.tree.scroll_offset(*root).is_some()),
            root: self.document_root(node).unwrap_or(node),
            source_anchor,
            source_anchor_end,
        });
        if external_text_input {
            self.clear_document_selection();
            self.tree.set_focused(Some(node));
            self.route_external_pointer(
                node,
                *pointer,
                aqua_ui::input::TextPointerPhase::Down {
                    click_count: count,
                    shift,
                },
                unit,
            );
        }
        true
    }

    fn route_external_pointer(
        &mut self,
        node: NodeId,
        pointer: aqua_core::PointerEvent,
        phase: aqua_ui::input::TextPointerPhase,
        unit: SelectionUnit,
    ) {
        let Some(bounds) = self.text_world_rect(node) else {
            return;
        };
        let at = Point::new(
            pointer
                .position
                .x
                .clamp(bounds.x, (bounds.right() - 0.1).max(bounds.x)),
            pointer
                .position
                .y
                .clamp(bounds.y, (bounds.bottom() - 0.1).max(bounds.y)),
        );
        let Some(position) = self.hit_text_position(node, at) else {
            return;
        };
        let byte = position.byte;
        let _ = unit;
        let host = &self.host;
        aqua_ui::input::route_text_pointer(
            &mut self.tree,
            node,
            pointer,
            byte,
            phase,
            |tree, target| host.input_allowed(tree, target),
        );
        self.tree.drain_commands();
    }

    fn selectable(&self, node: NodeId) -> bool {
        !self.tree.is_disabled(node) && self.tree.get(node).is_some_and(|record| matches!(&record.kind, NodeKind::Text(text) if text.selectable || text.is_editable()))
    }

    fn hit_text_position(&self, node: NodeId, at: Point) -> Option<CaretPosition> {
        let point = self.text_local_point(node, at)?;
        let entry = self.text_runs.get(&node)?;
        let guard = self.runs.lock();
        let layout = guard.resolve(entry.run)?.layout;
        layout.hit_position(point.x, point.y)
    }

    fn hit_text_word(&self, node: NodeId, at: Point) -> Option<usize> {
        let point = self.text_local_point(node, at)?;
        let entry = self.text_runs.get(&node)?;
        let guard = self.runs.lock();
        let layout = guard.resolve(entry.run)?.layout;
        layout.word_hit(point.x, point.y)
    }

    fn document_root(&self, mut node: NodeId) -> Option<NodeId> {
        loop {
            let record = self.tree.get(node)?;
            if matches!(record.kind, NodeKind::Scroll(_))
                || record.parent.is_none()
                || self.host.overlay_of_root(node).is_some()
            {
                return Some(node);
            }
            node = record.parent?;
        }
    }

    fn same_document(&self, left: NodeId, right: NodeId) -> bool {
        self.document_root(left).is_some() && self.document_root(left) == self.document_root(right)
    }

    fn document_nodes(&self, node: NodeId) -> Vec<NodeId> {
        let Some(root) = self.document_root(node) else {
            return Vec::new();
        };
        let mut stack = vec![root];
        let mut nodes = Vec::new();
        while let Some(candidate) = stack.pop() {
            let Some(record) = self.tree.get(candidate) else {
                continue;
            };
            if candidate != root && matches!(record.kind, NodeKind::Scroll(_)) {
                continue;
            }
            if self.text_node_allowed(candidate)
                && self.selectable(candidate)
                && matches!(&record.kind, NodeKind::Text(text) if !text.is_editable())
            {
                nodes.push(candidate);
            }
            stack.extend(record.children.iter().rev().copied());
        }
        nodes
    }

    fn make_document_index(&self, node: NodeId) -> Option<DocumentIndex> {
        let root = self.document_root(node)?;
        let nodes = self.document_nodes(node);
        let positions = nodes
            .iter()
            .enumerate()
            .map(|(position, node)| (*node, position))
            .collect();
        Some(DocumentIndex {
            root,
            version: self.tree.content_version(),
            nodes,
            positions,
        })
    }

    fn document_index(&mut self, node: NodeId) -> Option<Rc<DocumentIndex>> {
        let root = self.document_root(node)?;
        if let Some(index) = &self.interaction.document {
            if index.root == root && index.version == self.tree.content_version() {
                return Some(Rc::clone(index));
            }
        }
        let index = Rc::new(self.make_document_index(node)?);
        self.interaction.document = Some(Rc::clone(&index));
        Some(index)
    }

    fn document_range(
        &self,
        index: &DocumentIndex,
        selection: DocumentSelection,
        node: NodeId,
    ) -> Option<Range<usize>> {
        let NodeKind::Text(text) = &self.tree.get(node)?.kind else {
            return None;
        };
        let (start, end, bounds) = index.bounds(selection)?;
        if !bounds.contains(index.positions.get(&node)?) {
            return None;
        }
        Some(
            if node == start.node {
                start.byte.min(text.text().len())
            } else {
                0
            }..if node == end.node {
                end.byte.min(text.text().len())
            } else {
                text.text().len()
            },
        )
    }

    fn clear_document_selection(&mut self) {
        for node in std::mem::take(&mut self.interaction.selected_nodes) {
            self.tree.set_text_selection(node, None);
        }
        self.interaction.selection = None;
        self.interaction.source_document.clear_selection();
        self.interaction.source_owner = None;
    }

    fn set_document_selection(&mut self, selection: DocumentSelection) {
        if let (Some(anchor), Some(focus)) = (
            self.source_position(selection.anchor),
            self.source_position(selection.focus),
        ) {
            self.clear_document_selection();
            self.interaction.selection = Some(selection);
            self.interaction.source_owner = self.document_root(selection.anchor.node);
            self.interaction
                .source_document
                .set_selection(anchor, focus);
            self.paint_source_selection();
            return;
        }
        let Some(index) = self.document_index(selection.anchor.node) else {
            return;
        };
        let Some((_, _, bounds)) = index.bounds(selection) else {
            return;
        };
        self.clear_document_selection();
        self.interaction.selection = Some(selection);
        for node in index.nodes[bounds].iter().copied() {
            if self.interaction.source_slices.contains_key(&node) {
                continue;
            }
            if let Some(range) = self.document_range(&index, selection, node) {
                if !range.is_empty() {
                    self.tree
                        .set_text_selection(node, Some(Selection::new(range.start, range.end)));
                    self.interaction.selected_nodes.push(node);
                }
            }
        }
    }

    fn select_document(&mut self) -> bool {
        if self.interaction.source_document.selection().is_some() {
            if !self.source_selection_allowed() {
                return false;
            }
            let selected = self.interaction.source_document.select_all();
            self.paint_source_selection();
            return selected;
        }
        let Some(selection) = self.interaction.selection else {
            return false;
        };
        let nodes = self.document_nodes(selection.anchor.node);
        let (Some(&first), Some(&last)) = (nodes.first(), nodes.last()) else {
            return false;
        };
        let Some(NodeKind::Text(text)) = self.tree.get(last).map(|record| &record.kind) else {
            return false;
        };
        self.set_document_selection(DocumentSelection {
            anchor: TextPosition {
                node: first,
                byte: 0,
            },
            focus: TextPosition {
                node: last,
                byte: text.text().len(),
            },
        });
        true
    }

    fn extend_pointer_selection(&mut self, gesture: &Gesture, at: Point) {
        let node = if gesture.editable {
            gesture.anchor.node
        } else {
            let Some(index) = self.document_index(gesture.root) else {
                return;
            };
            let nodes = &index.nodes;
            let in_domain = |node: &NodeId| match gesture.source_anchor {
                Some(_) => {
                    self.interaction.source_slices.contains_key(node)
                        && self
                            .interaction
                            .source_scopes
                            .get(node)
                            .copied()
                            .unwrap_or_default()
                            == self.interaction.source_document_id
                }
                None => !self.interaction.source_slices.contains_key(node),
            };
            let hit = hit_test(&self.tree, at)
                .filter(|node| index.positions.contains_key(node) && in_domain(node));
            let nearest = || {
                nodes
                    .iter()
                    .copied()
                    .filter(in_domain)
                    .filter_map(|node| {
                        let rect = self.text_world_rect(node)?;
                        let dy = (rect.y - at.y).max(0.0).max(at.y - rect.bottom());
                        let dx = (rect.x - at.x).max(0.0).max(at.x - rect.right());
                        Some((node, dy * dy + dx * dx))
                    })
                    .min_by(|left, right| left.1.total_cmp(&right.1))
                    .map(|(node, _)| node)
            };
            let Some(node) = hit.or_else(nearest) else {
                return;
            };
            node
        };
        if !self.selectable(node) || !self.text_node_allowed(node) {
            return;
        }
        let Some(position) = self.hit_text_position(node, at) else {
            return;
        };
        let byte = if gesture.unit == SelectionUnit::Word {
            self.hit_text_word(node, at).unwrap_or(position.byte)
        } else {
            position.byte
        };
        let Some(NodeKind::Text(text)) = self.tree.get(node).map(|record| &record.kind) else {
            return;
        };
        let range = gesture.unit.drag_range_at(text.text(), byte);
        let source_focus = self.source_position(TextPosition { node, byte });
        let ordering = if let (Some(focus), Some(anchor)) = (source_focus, gesture.source_anchor) {
            self.interaction
                .source_document
                .compare_positions(focus, anchor)
        } else if node == gesture.anchor.node {
            Some(byte.cmp(&gesture.anchor.byte))
        } else {
            self.document_index(gesture.root)
                .and_then(|index| index.compare(TextPosition { node, byte }, gesture.anchor))
        };
        let before = match ordering {
            Some(Ordering::Less) => true,
            _ => node == gesture.anchor.node && byte < gesture.initial.start,
        };
        let anchor = if before {
            TextPosition {
                node: gesture.anchor.node,
                byte: gesture.initial.end,
            }
        } else {
            gesture.anchor
        };
        let focus = TextPosition {
            node,
            byte: if before { range.start } else { range.end },
        };
        if gesture.editable {
            self.tree
                .set_text_selection(node, Some(Selection::new(anchor.byte, focus.byte)));
            let target = CaretPosition {
                byte: focus.byte,
                affinity: if gesture.unit == SelectionUnit::Character {
                    position.affinity
                } else if before {
                    CaretAffinity::Downstream
                } else {
                    CaretAffinity::Upstream
                },
            };
            self.tree.edit_text(node, |text| {
                if let TextBacking::Editable(editor) = &mut text.backing {
                    editor.move_caret_position(target, true);
                }
            });
        } else if let (Some(source_anchor), Some(source_focus)) = (
            if before {
                gesture.source_anchor_end
            } else {
                gesture.source_anchor
            },
            self.source_position(focus),
        ) {
            self.interaction
                .source_document
                .set_selection(source_anchor, source_focus);
            self.interaction.selection = Some(DocumentSelection { anchor, focus });
            self.paint_source_selection();
        } else {
            self.set_document_selection(DocumentSelection { anchor, focus });
        }
    }

    fn gesture_allowed(&self, gesture: &Gesture) -> bool {
        if self.tree.get(gesture.root).is_none()
            || self.tree.is_disabled(gesture.root)
            || !self.text_node_allowed(gesture.root)
        {
            return false;
        }
        if self.tree.get(gesture.anchor.node).is_some() {
            return self.selectable(gesture.anchor.node)
                && self.text_node_allowed(gesture.anchor.node);
        }
        gesture.source_anchor.is_some_and(|anchor| {
            self.interaction
                .source_document
                .source(anchor.source)
                .is_some()
        })
    }

    fn selection_scroll(&self, gesture: &Gesture) -> Option<(NodeId, Point)> {
        if !gesture.dragged || !self.gesture_allowed(gesture) {
            return None;
        }
        if !gesture.selection_auto_scroll {
            return None;
        }
        let node = gesture.scroll?;
        let bounds = self.tree.rect(node)?;
        let padding = self.tree.get(node)?.style.padding;
        let rect = Rect::from_edges(
            padding.left,
            padding.top,
            bounds.w - padding.right,
            bounds.h - padding.bottom,
        );
        let pointer = aqua_ui::input::local_position(&self.tree, node, gesture.pointer)?;
        let offset = self.tree.scroll_offset(node)?;
        let content = self.tree.content_size(node)?;
        let velocity = |coordinate: f32, start: f32, end: f32, current: f32, maximum: f32| {
            let penetration = if coordinate < start + 20.0 {
                coordinate - start - 20.0
            } else if coordinate > end - 20.0 {
                coordinate - end + 20.0
            } else {
                0.0
            };
            let velocity = (penetration * 12.0).clamp(-1200.0, 1200.0);
            if (velocity < 0.0 && current <= 0.0) || (velocity > 0.0 && current >= maximum) {
                0.0
            } else {
                velocity
            }
        };
        let velocity = Point::new(
            velocity(
                pointer.x,
                rect.x,
                rect.right(),
                offset.x,
                (content.w - rect.w).max(0.0),
            ),
            velocity(
                pointer.y,
                rect.y,
                rect.bottom(),
                offset.y,
                (content.h - rect.h).max(0.0),
            ),
        );
        (velocity.x != 0.0 || velocity.y != 0.0).then_some((node, velocity))
    }

    pub(super) fn extend_selection_after_scroll(&mut self) {
        if let Some(gesture) = self.interaction.gesture.clone() {
            if gesture.dragged && self.gesture_allowed(&gesture) {
                self.extend_pointer_selection(&gesture, gesture.pointer);
            }
        }
    }

    pub(super) fn selection_deadline(&self) -> Option<f64> {
        let gesture = self.interaction.gesture.as_ref()?;
        self.selection_scroll(gesture)?;
        Some(self.clock + 1.0 / 60.0)
    }

    pub(super) fn advance_text_selection(&mut self) -> bool {
        let Some(mut gesture) = self.interaction.gesture.clone() else {
            return false;
        };
        if !self.gesture_allowed(&gesture) {
            self.interaction.gesture = None;
            self.tree.take_press(gesture.contact);
            return false;
        }
        let elapsed = (self.clock - gesture.advanced_at).clamp(0.0, 0.1) as f32;
        gesture.advanced_at = self.clock;
        let Some((node, velocity)) = self.selection_scroll(&gesture) else {
            self.interaction.gesture = Some(gesture);
            return false;
        };
        let Some(offset) = self.tree.scroll_offset(node) else {
            return false;
        };
        let changed = elapsed > 0.0
            && self.tree.scroll_to(
                node,
                Point::new(
                    offset.x + velocity.x * elapsed,
                    offset.y + velocity.y * elapsed,
                ),
            );
        if changed {
            self.extend_pointer_selection(&gesture, gesture.pointer);
        }
        self.interaction.gesture = Some(gesture);
        changed
    }

    fn navigate_text(&mut self, key: &KeyEvent) -> bool {
        if key.modifiers.contains(Modifiers::ALT) || key.modifiers.contains(Modifiers::LOGO) {
            return false;
        }
        let Key::Named(
            named @ (NamedKey::Left
            | NamedKey::Right
            | NamedKey::Up
            | NamedKey::Down
            | NamedKey::PageUp
            | NamedKey::PageDown
            | NamedKey::Home
            | NamedKey::End),
        ) = key.key
        else {
            return false;
        };
        let paragraph = key.modifiers.contains(Modifiers::CONTROL);
        if paragraph && !matches!(named, NamedKey::Up | NamedKey::Down) {
            return false;
        }
        let Some(node) = self.editable_target() else {
            return false;
        };
        self.sync_text_runs_for(Some(node));
        let Some(NodeKind::Text(text)) = self.tree.get(node).map(|record| &record.kind) else {
            return false;
        };
        let external_text_input = text.external_text_input;
        let TextBacking::Editable(editor) = &text.backing else {
            return false;
        };
        if external_text_input && (!matches!(named, NamedKey::Up | NamedKey::Down) || paragraph) {
            return false;
        }
        if editor.is_composing() && !external_text_input {
            return false;
        }
        if external_text_input
            && self.interaction.external_vertical_revision != Some((node, editor.revision()))
        {
            self.interaction.goal_x = None;
        }
        let selection = editor.selection();
        let position = if external_text_input && editor.is_composing() {
            CaretPosition {
                byte: editor.ime_caret(),
                affinity: CaretAffinity::Downstream,
            }
        } else {
            editor.caret_position()
        };
        if matches!(named, NamedKey::Left | NamedKey::Right)
            && !selection.is_caret()
            && !key.modifiers.contains(Modifiers::SHIFT)
        {
            return false;
        }
        let Some(entry) = self.text_runs.get(&node) else {
            return false;
        };
        let guard = self.runs.lock();
        let Some(handle) = guard.resolve(entry.run) else {
            return false;
        };
        let layout = handle.layout;
        let Some(caret) = layout.caret_position(position) else {
            return false;
        };
        let goal_x = self
            .interaction
            .goal_x
            .filter(|(owner, _)| *owner == node)
            .map_or(caret.x, |(_, x)| x);
        let up = matches!(named, NamedKey::Up | NamedKey::PageUp);
        let target = match named {
            NamedKey::Left | NamedKey::Right => {
                layout.horizontal_position(position, named == NamedKey::Left)
            }
            NamedKey::Up | NamedKey::Down if paragraph => {
                layout.paragraph_position(position, up, goal_x)
            }
            NamedKey::Home | NamedKey::End => {
                layout.line_boundary_position(position, named == NamedKey::End)
            }
            NamedKey::PageUp | NamedKey::PageDown => {
                let distance =
                    self.text_rect(node).map_or(self.viewport.h, |rect| rect.h) * self.scale;
                let y = caret.y_top + caret.height * 0.5 + if up { -distance } else { distance };
                layout.hit_position(goal_x, y)
            }
            _ => layout.vertical_position(position, up, goal_x),
        };
        drop(guard);
        let Some(target) = target else {
            return false;
        };
        let extend = key.modifiers.contains(Modifiers::SHIFT);
        if external_text_input {
            self.interaction.goal_x = Some((node, goal_x));
            let host = &self.host;
            aqua_ui::input::route_text_vertical(
                &mut self.tree,
                node,
                up,
                extend,
                target.byte,
                |tree, target| host.input_allowed(tree, target),
            );
            self.tree.drain_commands();
            self.interaction.external_vertical_revision = self
                .tree
                .editor_snapshot(node)
                .map(|snapshot| (node, snapshot.revision));
            return true;
        }
        self.tree.set_text_selection(
            node,
            Some(Selection::new(
                if extend {
                    selection.anchor
                } else {
                    target.byte
                },
                target.byte,
            )),
        );
        self.tree.edit_text(node, |text| {
            if let TextBacking::Editable(editor) = &mut text.backing {
                editor.move_caret_position(target, true);
            }
        });
        self.interaction.goal_x = (!matches!(
            named,
            NamedKey::Left | NamedKey::Right | NamedKey::Home | NamedKey::End
        ))
        .then_some((node, goal_x));
        true
    }
}
