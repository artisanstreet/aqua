use std::collections::HashMap;
use std::rc::Rc;

use aqua_core::{CursorIcon, InputEvent, Key, Modifiers, NodeId, Point, PointerEvent, Rect};

use crate::events::hit_test;
use crate::tree::{ActionEvent, ActionKind, UiCommand, UiTree};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasteImage {
    width: usize,
    height: usize,
    rgba: std::sync::Arc<[u8]>,
}

impl PasteImage {
    pub fn new(width: usize, height: usize, rgba: std::sync::Arc<[u8]>) -> Option<Self> {
        (width > 0
            && height > 0
            && width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(4))
                == Some(rgba.len()))
        .then_some(Self {
            width,
            height,
            rgba,
        })
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardPaste {
    pub text: Option<std::sync::Arc<str>>,
    pub image: Option<PasteImage>,
    pub paths: std::sync::Arc<[std::path::PathBuf]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextEdit {
    Insert(String),
    Paste(String),
    ReplaceRange {
        range: std::ops::Range<usize>,
        text: String,
    },
    SetSelection(aqua_text::Selection),
    SetExternalProjection {
        text: String,
        selection: aqua_text::Selection,
        marked: Option<std::ops::Range<usize>>,
    },
    Undo,
    Redo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorSnapshot {
    pub text: String,
    pub selection: aqua_text::Selection,
    pub revision: u64,
    pub multiline: bool,
    pub composing: bool,
}

impl UiTree {
    pub fn editor_snapshot(&self, node: NodeId) -> Option<EditorSnapshot> {
        let crate::tree::NodeKind::Text(text) = &self.get(node)?.kind else {
            return None;
        };
        let crate::tree::TextBacking::Editable(editor) = &text.backing else {
            return None;
        };
        Some(EditorSnapshot {
            text: editor.text().to_owned(),
            selection: editor.selection(),
            revision: editor.revision(),
            multiline: text.multiline(),
            composing: editor.is_composing(),
        })
    }

    pub fn apply_text_edit(&mut self, node: NodeId, revision: Option<u64>, edit: TextEdit) -> bool {
        if self.is_disabled(node) {
            return false;
        }
        let Some(snapshot) = self.editor_snapshot(node) else {
            return false;
        };
        let external_projection = matches!(&edit, TextEdit::SetExternalProjection { .. });
        let allowed_external = self.get(node).is_some_and(|record| {
            matches!(
                &record.kind, crate::tree::NodeKind::Text(text) if text.external_text_input
            )
        });
        if (external_projection != allowed_external)
            || (snapshot.composing && !external_projection)
            || revision.is_some_and(|revision| revision != snapshot.revision)
        {
            return false;
        }
        let changed = self
            .edit_editor(node, |editor| {
                let before = editor.revision();
                match edit {
                    TextEdit::Insert(text) => {
                        editor.break_typing_group();
                        editor.insert(&text);
                        editor.break_typing_group();
                    }
                    TextEdit::Paste(text) => {
                        editor.insert_paste(&text);
                    }
                    TextEdit::ReplaceRange { range, text } => {
                        return editor.replace_range(range, &text);
                    }
                    TextEdit::SetSelection(selection) => {
                        editor.set_selection(selection);
                        return true;
                    }
                    TextEdit::SetExternalProjection {
                        text,
                        selection,
                        marked,
                    } => {
                        return editor.set_external_projection(&text, selection, marked);
                    }
                    TextEdit::Undo => {
                        editor.undo();
                    }
                    TextEdit::Redo => {
                        editor.redo();
                    }
                }
                editor.revision() != before
            })
            .unwrap_or(false);
        if let Some(after) = self.editor_snapshot(node) {
            if !external_projection && after.revision != snapshot.revision {
                self.fire_action(ActionEvent {
                    source: None,
                    node,
                    kind: ActionKind::TextChanged,
                    text: Some(after.text),
                    focused: None,
                });
            }
        }
        changed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputKind {
    PointerDown,
    PointerUp,
    PointerMove,
    PointerEnter,
    PointerLeave,
    PointerCancel,
    Scroll,
    KeyDown,
    KeyUp,
    TextInput,
    Ime,
    Focus,
    Modifiers,
    TextRangeActivated,
    TextPointer,
    TextContext,
    TextVertical,
    FilesDropped,
    ClipboardPaste,
    ClipboardCut,
    DragEnter,
    DragOver,
    DragLeave,
    Drop,
    DragEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventPhase {
    Capture,
    Bubble,
}

#[derive(Clone)]
pub struct DragPayload {
    value: Rc<dyn std::any::Any>,
    name: &'static str,
}

impl DragPayload {
    pub fn new<T: 'static>(value: T) -> Self {
        Self {
            value: Rc::new(value),
            name: std::any::type_name::<T>(),
        }
    }
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.value.downcast_ref::<T>()
    }
}

impl std::fmt::Debug for DragPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("DragPayload")
            .field(&self.name)
            .finish()
    }
}

impl PartialEq for DragPayload {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.value, &other.value)
    }
}

#[derive(Clone, Debug, Default)]
pub enum DragRequest {
    #[default]
    None,
    Start(DragPayload),
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPointerPhase {
    Down { click_count: usize, shift: bool },
    Drag,
    Up,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RoutedInput {
    ClipboardPaste(ClipboardPaste),
    ClipboardCut,
    TextVertical {
        up: bool,
        extend: bool,
        target: usize,
    },
    Native(InputEvent),
    PointerEnter(PointerEvent),
    PointerLeave(PointerEvent),
    TextPointer {
        pointer: PointerEvent,
        byte: usize,
        phase: TextPointerPhase,
    },
    TextContext {
        pointer: PointerEvent,
        byte: usize,
    },
    TextRangeActivated {
        id: u64,
        range: std::ops::Range<usize>,
        pointer: PointerEvent,
    },
    FilesDropped {
        paths: std::sync::Arc<[std::path::PathBuf]>,
        at: Option<Point>,
    },
    DragEnter {
        payload: DragPayload,
        pointer: PointerEvent,
    },
    DragOver {
        payload: DragPayload,
        pointer: PointerEvent,
    },
    DragLeave {
        payload: DragPayload,
        pointer: PointerEvent,
    },
    Drop {
        payload: DragPayload,
        pointer: PointerEvent,
    },
    DragEnd {
        payload: DragPayload,
        pointer: PointerEvent,
        accepted: bool,
    },
}

impl RoutedInput {
    pub fn kind(&self) -> InputKind {
        match self {
            Self::ClipboardPaste(_) => InputKind::ClipboardPaste,
            Self::ClipboardCut => InputKind::ClipboardCut,
            Self::TextVertical { .. } => InputKind::TextVertical,
            Self::PointerEnter(_) => InputKind::PointerEnter,
            Self::PointerLeave(_) => InputKind::PointerLeave,
            Self::TextRangeActivated { .. } => InputKind::TextRangeActivated,
            Self::TextPointer { .. } => InputKind::TextPointer,
            Self::TextContext { .. } => InputKind::TextContext,
            Self::FilesDropped { .. } => InputKind::FilesDropped,
            Self::DragEnter { .. } => InputKind::DragEnter,
            Self::DragOver { .. } => InputKind::DragOver,
            Self::DragLeave { .. } => InputKind::DragLeave,
            Self::Drop { .. } => InputKind::Drop,
            Self::DragEnd { .. } => InputKind::DragEnd,
            Self::Native(input) => match input {
                InputEvent::PointerDown(..) => InputKind::PointerDown,
                InputEvent::PointerUp(..) => InputKind::PointerUp,
                InputEvent::PointerMove(_) => InputKind::PointerMove,
                InputEvent::PointerCancel { .. } => InputKind::PointerCancel,
                InputEvent::Scroll { .. } => InputKind::Scroll,
                InputEvent::KeyDown(_) => InputKind::KeyDown,
                InputEvent::KeyUp(_) => InputKind::KeyUp,
                InputEvent::TextInput { .. } => InputKind::TextInput,
                InputEvent::Ime(_)
                | InputEvent::ImeReplace { .. }
                | InputEvent::ImeSelect { .. } => InputKind::Ime,
                InputEvent::Focused(_) => InputKind::Focus,
                InputEvent::ModifiersChanged(_) => InputKind::Modifiers,
            },
        }
    }

    pub fn pointer(&self) -> Option<PointerEvent> {
        match self {
            Self::Native(
                InputEvent::PointerDown(pointer, _)
                | InputEvent::PointerUp(pointer, _)
                | InputEvent::PointerMove(pointer),
            )
            | Self::PointerEnter(pointer)
            | Self::PointerLeave(pointer)
            | Self::TextRangeActivated { pointer, .. }
            | Self::TextContext { pointer, .. }
            | Self::TextPointer { pointer, .. } => Some(*pointer),
            Self::DragEnter { pointer, .. }
            | Self::DragOver { pointer, .. }
            | Self::DragLeave { pointer, .. }
            | Self::Drop { pointer, .. }
            | Self::DragEnd { pointer, .. } => Some(*pointer),
            _ => None,
        }
    }

    pub fn position(&self) -> Option<Point> {
        self.pointer()
            .map(|pointer| pointer.position)
            .or(match self {
                Self::Native(InputEvent::Scroll { at, .. }) => Some(*at),
                Self::FilesDropped { at, .. } => *at,
                _ => None,
            })
    }
}

#[derive(Clone, Debug)]
pub struct RoutedEvent {
    pub target: NodeId,
    pub current_target: NodeId,
    pub phase: EventPhase,
    pub input: RoutedInput,
    pub modifiers: Modifiers,
    pub click_count: Option<usize>,
    pub bounds: Rect,
    pub local_position: Option<Point>,
    pub editor: Option<EditorSnapshot>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureRequest {
    #[default]
    None,
    Capture,
    Release,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusRequest {
    #[default]
    None,
    Focus,
    Blur,
}

#[derive(Debug, Default)]
pub struct InputResponse {
    pub handled: bool,
    pub prevent_default: bool,
    pub stop_propagation: bool,
    pub pointer_capture: CaptureRequest,
    pub focus: FocusRequest,
    pub commands: Vec<UiCommand>,
    pub drag: DragRequest,
    pub accept_drop: bool,
}

impl InputResponse {
    pub fn handled() -> Self {
        Self {
            handled: true,
            prevent_default: true,
            ..Self::default()
        }
    }

    pub fn capture() -> Self {
        Self {
            pointer_capture: CaptureRequest::Capture,
            ..Self::handled()
        }
    }
}

pub type InputCallback = Rc<dyn Fn(&RoutedEvent) -> InputResponse>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: Key,
    pub modifiers: Modifiers,
    pub action: ActionKind,
    pub repeat: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoutedOutcome {
    pub target: Option<NodeId>,
    pub handled: bool,
    pub default_prevented: bool,
    pub propagation_stopped: bool,
    pub drop_accepted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BoundInput {
    Listener(InputKind, EventPhase),
    Cursor,
    Shortcut(Key, Modifiers),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DragSnapshot {
    pub source: NodeId,
    pub payload: DragPayload,
    pub pointer: PointerEvent,
    pub cursor_offset: Point,
}

struct DragSession {
    source: NodeId,
    payload: DragPayload,
    pointer: PointerEvent,
    cursor_offset: Point,
    target: Option<NodeId>,
    accepted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExternalFileDragEvent {
    Enter {
        paths: Vec<std::path::PathBuf>,
        position: Point,
    },
    Move {
        position: Point,
    },
    Submit {
        position: Point,
    },
    Exit,
}

struct ExternalFileDragSession {
    paths: std::sync::Arc<[std::path::PathBuf]>,
    payload: DragPayload,
    pointer: PointerEvent,
    target: Option<NodeId>,
}

#[derive(Default)]
pub(crate) struct InputRouter {
    listeners: HashMap<(NodeId, InputKind, EventPhase), InputCallback>,
    captures: HashMap<u64, NodeId>,
    hovered: Vec<NodeId>,
    cursors: HashMap<NodeId, CursorIcon>,
    shortcuts: HashMap<NodeId, Vec<Shortcut>>,
    modifiers: Modifiers,
    bound: HashMap<NodeId, Vec<BoundInput>>,
    last_pointer: Option<PointerEvent>,
    drag: Option<DragSession>,
    external_file_drag: Option<ExternalFileDragSession>,
}

impl InputRouter {
    pub(crate) fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    pub(crate) fn remove_node(&mut self, node: NodeId) {
        self.listeners.retain(|(id, _, _), _| *id != node);
        self.captures.retain(|_, id| *id != node);
        self.hovered.retain(|id| *id != node);
        self.cursors.remove(&node);
        self.shortcuts.remove(&node);
        self.bound.remove(&node);
    }
}

impl UiTree {
    pub(crate) fn cancel_input_node(&mut self, node: NodeId) {
        if self
            .input
            .drag
            .as_ref()
            .is_some_and(|drag| drag.source == node)
        {
            finish_drag(self, false);
        }
        if self
            .input
            .drag
            .as_ref()
            .is_some_and(|drag| drag.target == Some(node))
        {
            if let Some(mut drag) = self.input.drag.take() {
                route(
                    self,
                    node,
                    RoutedInput::DragLeave {
                        payload: drag.payload.clone(),
                        pointer: drag.pointer,
                    },
                    &|_, _| true,
                );
                drag.target = None;
                drag.accepted = false;
                self.input.drag = Some(drag);
            }
        }
        let contacts: Vec<_> = self
            .input
            .captures
            .iter()
            .filter(|(_, owner)| **owner == node)
            .map(|(contact, _)| *contact)
            .collect();
        for contact in contacts {
            route(
                self,
                node,
                RoutedInput::Native(InputEvent::PointerCancel { contact }),
                &|_, _| true,
            );
        }
        self.input.remove_node(node);
    }

    pub fn clear_input_bindings(&mut self, node: NodeId) {
        let Some(bindings) = self.input.bound.remove(&node) else {
            return;
        };
        if bindings
            .iter()
            .any(|binding| matches!(binding, BoundInput::Listener(..)))
        {
            let contacts: Vec<_> = self
                .input
                .captures
                .iter()
                .filter(|(_, owner)| **owner == node)
                .map(|(contact, _)| *contact)
                .collect();
            for contact in contacts {
                route(
                    self,
                    node,
                    RoutedInput::Native(InputEvent::PointerCancel { contact }),
                    &|_, _| true,
                );
                self.input.captures.remove(&contact);
            }
        }
        for binding in bindings {
            match binding {
                BoundInput::Listener(kind, phase) => {
                    self.input.listeners.remove(&(node, kind, phase));
                }
                BoundInput::Cursor => {
                    self.input.cursors.remove(&node);
                }
                BoundInput::Shortcut(key, modifiers) => {
                    if let Some(shortcuts) = self.input.shortcuts.get_mut(&node) {
                        shortcuts.retain(|shortcut| {
                            shortcut.key != key || shortcut.modifiers != modifiers
                        });
                    }
                }
            }
        }
    }

    pub fn listen_input_bound(
        &mut self,
        node: NodeId,
        kind: InputKind,
        phase: EventPhase,
        callback: InputCallback,
    ) -> bool {
        let registered = self.listen_input(node, kind, phase, callback);
        if registered {
            self.invalidate_bindings(node);
            self.input
                .bound
                .entry(node)
                .or_default()
                .push(BoundInput::Listener(kind, phase));
        }
        registered
    }

    pub fn set_cursor_bound(&mut self, node: NodeId, cursor: CursorIcon) -> bool {
        let registered = self.set_cursor(node, cursor);
        if registered {
            self.invalidate_bindings(node);
            self.input
                .bound
                .entry(node)
                .or_default()
                .push(BoundInput::Cursor);
        }
        registered
    }

    pub fn bind_shortcut_bound(&mut self, node: NodeId, shortcut: Shortcut) -> bool {
        let registered = self.bind_shortcut(node, shortcut);
        if registered {
            self.invalidate_bindings(node);
            self.input
                .bound
                .entry(node)
                .or_default()
                .push(BoundInput::Shortcut(shortcut.key, shortcut.modifiers));
        }
        registered
    }

    pub fn listen_input(
        &mut self,
        node: NodeId,
        kind: InputKind,
        phase: EventPhase,
        callback: InputCallback,
    ) -> bool {
        if self.get(node).is_none() {
            return false;
        }
        if let Some(bound) = self.input.bound.get_mut(&node) {
            bound.retain(|binding| *binding != BoundInput::Listener(kind, phase));
        }
        self.input.listeners.insert((node, kind, phase), callback);
        true
    }

    pub fn remove_input_listener(&mut self, node: NodeId, kind: InputKind, phase: EventPhase) {
        self.input.listeners.remove(&(node, kind, phase));
    }

    pub fn set_cursor(&mut self, node: NodeId, cursor: CursorIcon) -> bool {
        if self.get(node).is_none() {
            return false;
        }
        if let Some(bound) = self.input.bound.get_mut(&node) {
            bound.retain(|binding| *binding != BoundInput::Cursor);
        }
        self.input.cursors.insert(node, cursor);
        true
    }

    pub fn cursor(&self) -> CursorIcon {
        let captured = self
            .input
            .captures
            .values()
            .next()
            .copied()
            .or_else(|| crate::controls::captured_node(self))
            .or_else(|| self.input.drag.as_ref().map(|drag| drag.source));
        let path = captured
            .map(|node| event_path(self, node))
            .unwrap_or_else(|| self.input.hovered.clone());
        let cursor = path.iter().find_map(|node| {
            self.input.cursors.get(node).copied().or_else(|| {
                self.get(*node).and_then(|record| match &record.kind {
                    crate::tree::NodeKind::Text(text) if text.selectable || text.is_editable() => {
                        Some(CursorIcon::Text)
                    }
                    _ => None,
                })
            })
        });
        match (cursor, self.input.drag.as_ref(), captured) {
            (Some(CursorIcon::Grab), _, Some(_)) => CursorIcon::Grabbing,
            (Some(cursor), _, _) => cursor,
            (None, Some(drag), _) => {
                if drag.accepted {
                    CursorIcon::Move
                } else {
                    CursorIcon::NotAllowed
                }
            }
            _ => CursorIcon::Default,
        }
    }

    pub fn input_is_captured(&self) -> bool {
        !self.input.captures.is_empty()
            || self.input.drag.is_some()
            || crate::controls::captured_node(self).is_some()
    }

    pub fn pointer_position(&self) -> Option<Point> {
        self.input.last_pointer.map(|pointer| pointer.position)
    }

    pub fn drag_snapshot(&self) -> Option<DragSnapshot> {
        let drag = self.input.drag.as_ref()?;
        Some(DragSnapshot {
            source: drag.source,
            payload: drag.payload.clone(),
            pointer: drag.pointer,
            cursor_offset: drag.cursor_offset,
        })
    }

    pub fn pointer_capture(&self, contact: u64) -> Option<NodeId> {
        self.input
            .captures
            .get(&contact)
            .copied()
            .or_else(|| crate::controls::pointer_capture(self, contact))
    }

    pub fn bind_shortcut(&mut self, node: NodeId, shortcut: Shortcut) -> bool {
        if self.get(node).is_none() {
            return false;
        }
        if let Some(bound) = self.input.bound.get_mut(&node) {
            bound.retain(|binding| {
                *binding != BoundInput::Shortcut(shortcut.key, shortcut.modifiers)
            });
        }
        let bindings = self.input.shortcuts.entry(node).or_default();
        bindings.retain(|binding| {
            !keys_match(binding.key, shortcut.key) || binding.modifiers != shortcut.modifiers
        });
        bindings.push(shortcut);
        true
    }
}

pub fn visual_bounds(tree: &UiTree, node: NodeId) -> Option<Rect> {
    tree.visual_rect(node)
}

pub fn local_position(tree: &UiTree, node: NodeId, point: Point) -> Option<Point> {
    let transform = tree.visual_transform(node)?;
    let determinant = transform.a * transform.d - transform.b * transform.c;
    if !determinant.is_finite() || determinant.abs() <= f32::EPSILON {
        return None;
    }
    let x = point.x - transform.tx;
    let y = point.y - transform.ty;
    Some(Point::new(
        (transform.d * x - transform.c * y) / determinant,
        (-transform.b * x + transform.a * y) / determinant,
    ))
}

fn keys_match(left: Key, right: Key) -> bool {
    match (left, right) {
        (Key::Char(left), Key::Char(right)) => left.eq_ignore_ascii_case(&right),
        _ => left == right,
    }
}

fn event_path(tree: &UiTree, mut node: NodeId) -> Vec<NodeId> {
    let mut path = Vec::new();
    while let Some(record) = tree.get(node) {
        path.push(node);
        let Some(parent) = record.parent else {
            break;
        };
        node = parent;
    }
    path
}

pub fn dispatch_action(tree: &UiTree, target: NodeId, kind: ActionKind) -> bool {
    dispatch_action_allowed(tree, target, kind, None, &|_, _| true)
}

fn dispatch_action_allowed(
    tree: &UiTree,
    target: NodeId,
    kind: ActionKind,
    source: Option<crate::tree::ActivationSource>,
    allowed: &dyn Fn(&UiTree, NodeId) -> bool,
) -> bool {
    for node in event_path(tree, target) {
        if tree.is_disabled(node) || !allowed(tree, node) {
            continue;
        }
        if tree.has_action(node, kind) {
            tree.fire_action(ActionEvent {
                source,
                node,
                kind,
                text: None,
                focused: None,
            });
            return true;
        }
    }
    false
}

fn route(
    tree: &mut UiTree,
    target: NodeId,
    input: RoutedInput,
    allowed: &dyn Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    let mut result = RoutedOutcome {
        target: Some(target),
        ..RoutedOutcome::default()
    };
    let cancel = matches!(
        &input,
        RoutedInput::Native(InputEvent::PointerCancel { .. })
            | RoutedInput::PointerLeave(_)
            | RoutedInput::DragEnd { .. }
            | RoutedInput::DragLeave { .. }
    );
    let path = if matches!(
        &input,
        RoutedInput::PointerEnter(_) | RoutedInput::PointerLeave(_)
    ) {
        vec![target]
    } else {
        event_path(tree, target)
    };
    for phase in [EventPhase::Capture, EventPhase::Bubble] {
        let ordered: Vec<_> = match phase {
            EventPhase::Capture => path.iter().rev().copied().collect(),
            EventPhase::Bubble => path.clone(),
        };
        for node in ordered {
            if (!cancel && tree.is_disabled(node)) || !allowed(tree, node) {
                continue;
            }
            let Some(callback) = tree
                .input
                .listeners
                .get(&(node, input.kind(), phase))
                .cloned()
            else {
                continue;
            };
            let bounds = visual_bounds(tree, node).unwrap_or(Rect::ZERO);
            let event = RoutedEvent {
                target,
                current_target: node,
                phase,
                modifiers: tree.input.modifiers,
                click_count: match &input {
                    RoutedInput::Native(InputEvent::PointerDown(pointer, button)) => {
                        tree.interaction_click_count(pointer, *button)
                    }
                    _ => None,
                },
                bounds,
                local_position: input
                    .position()
                    .and_then(|point| local_position(tree, node, point)),
                editor: tree.editor_snapshot(target),
                input: input.clone(),
            };
            let response = callback(&event);
            result.handled |= response.handled;
            result.default_prevented |= response.prevent_default;
            result.propagation_stopped |= response.stop_propagation;
            result.drop_accepted |= response.accept_drop;
            match response.drag {
                DragRequest::None => {}
                DragRequest::Cancel => finish_drag(tree, false),
                DragRequest::Start(payload) => {
                    if let Some(pointer) = input.pointer() {
                        finish_drag(tree, false);
                        tree.queue_interaction_event(
                            node,
                            crate::tree::interaction::InteractionEvent::Dragging(true),
                        );
                        tree.input.drag = Some(DragSession {
                            source: node,
                            payload,
                            pointer,
                            cursor_offset: Point::new(
                                pointer.position.x - bounds.x,
                                pointer.position.y - bounds.y,
                            ),
                            target: None,
                            accepted: false,
                        });
                        result.default_prevented = true;
                        result.handled = true;
                    }
                }
            }
            if let Some(pointer) = input.pointer() {
                match response.pointer_capture {
                    CaptureRequest::None => {}
                    CaptureRequest::Capture => {
                        let previous = tree.input.captures.insert(pointer.contact, node);
                        tree.queue_interaction_event(
                            node,
                            crate::tree::interaction::InteractionEvent::Capture(true),
                        );
                        if let Some(previous) = previous.filter(|previous| *previous != node) {
                            tree.queue_interaction_event(
                                previous,
                                crate::tree::interaction::InteractionEvent::Capture(false),
                            );
                            route(
                                tree,
                                previous,
                                RoutedInput::Native(InputEvent::PointerCancel {
                                    contact: pointer.contact,
                                }),
                                &|_, _| true,
                            );
                        }
                    }
                    CaptureRequest::Release => {
                        if tree.input.captures.get(&pointer.contact) == Some(&node) {
                            tree.input.captures.remove(&pointer.contact);
                            tree.queue_interaction_event(
                                node,
                                crate::tree::interaction::InteractionEvent::Capture(false),
                            );
                        }
                    }
                }
            }
            match response.focus {
                FocusRequest::None => {}
                FocusRequest::Focus => {
                    tree.set_focused(Some(node));
                }
                FocusRequest::Blur => {
                    tree.set_focused(None);
                }
            }
            for command in response.commands {
                if let UiCommand::EditText { node, .. } = &command {
                    if tree.is_disabled(*node) || !allowed(tree, *node) {
                        continue;
                    }
                }
                tree.command_sink().push(command);
            }
            if result.propagation_stopped {
                return result;
            }
        }
    }
    result
}

fn finish_drag(tree: &mut UiTree, accepted: bool) {
    let Some(drag) = tree.input.drag.take() else {
        return;
    };
    if let Some(target) = drag.target {
        route(
            tree,
            target,
            RoutedInput::DragLeave {
                payload: drag.payload.clone(),
                pointer: drag.pointer,
            },
            &|_, _| true,
        );
    }
    let contact = drag.pointer.contact;
    tree.queue_interaction_event(
        drag.source,
        crate::tree::interaction::InteractionEvent::Dragging(false),
    );
    route(
        tree,
        drag.source,
        RoutedInput::DragEnd {
            payload: drag.payload,
            pointer: drag.pointer,
            accepted,
        },
        &|_, _| true,
    );
    tree.input.captures.remove(&contact);
}

fn move_drag(
    tree: &mut UiTree,
    drag: &mut DragSession,
    pointer: PointerEvent,
    allowed: &dyn Fn(&UiTree, NodeId) -> bool,
) {
    drag.pointer = pointer;
    let target = hit_test(tree, pointer.position)
        .filter(|node| !tree.is_disabled(*node) && allowed(tree, *node));
    if target != drag.target {
        if let Some(previous) = drag.target {
            route(
                tree,
                previous,
                RoutedInput::DragLeave {
                    payload: drag.payload.clone(),
                    pointer,
                },
                &|_, _| true,
            );
        }
        if let Some(target) = target {
            route(
                tree,
                target,
                RoutedInput::DragEnter {
                    payload: drag.payload.clone(),
                    pointer,
                },
                allowed,
            );
        }
        drag.target = target;
    }
    drag.accepted = target.is_some_and(|target| {
        route(
            tree,
            target,
            RoutedInput::DragOver {
                payload: drag.payload.clone(),
                pointer,
            },
            allowed,
        )
        .drop_accepted
    });
}

pub fn cancel_blocked(tree: &mut UiTree, allowed: impl Fn(&UiTree, NodeId) -> bool) {
    let blocked: Vec<_> = tree
        .input
        .captures
        .iter()
        .filter(|(_, node)| {
            tree.get(**node).is_none() || tree.is_disabled(**node) || !allowed(tree, **node)
        })
        .map(|(contact, node)| (*contact, *node))
        .collect();
    for (contact, node) in blocked {
        route(
            tree,
            node,
            RoutedInput::Native(InputEvent::PointerCancel { contact }),
            &|_, _| true,
        );
        tree.input.captures.remove(&contact);
    }
    if tree.input.drag.as_ref().is_some_and(|drag| {
        tree.get(drag.source).is_none()
            || tree.is_disabled(drag.source)
            || !allowed(tree, drag.source)
    }) {
        finish_drag(tree, false);
    }
    let blocked: Vec<_> = tree
        .input
        .hovered
        .iter()
        .copied()
        .filter(|node| {
            tree.get(*node).is_none() || tree.is_disabled(*node) || !allowed(tree, *node)
        })
        .collect();
    if let Some(pointer) = tree.input.last_pointer {
        for node in &blocked {
            route(tree, *node, RoutedInput::PointerLeave(pointer), &|_, _| {
                true
            });
        }
    }
    tree.input.hovered.retain(|node| !blocked.contains(node));
}

pub fn route_text_vertical(
    tree: &mut UiTree,
    node: NodeId,
    up: bool,
    extend: bool,
    target: usize,
    allowed: impl Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    if tree.is_disabled(node) || !allowed(tree, node) {
        return RoutedOutcome::default();
    }
    route(
        tree,
        node,
        RoutedInput::TextVertical { up, extend, target },
        &allowed,
    )
}

pub fn route_text_pointer(
    tree: &mut UiTree,
    node: NodeId,
    pointer: PointerEvent,
    byte: usize,
    phase: TextPointerPhase,
    allowed: impl Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    if tree.is_disabled(node) || !allowed(tree, node) {
        return RoutedOutcome::default();
    }
    route(
        tree,
        node,
        RoutedInput::TextPointer {
            pointer,
            byte,
            phase,
        },
        &allowed,
    )
}

pub fn route_text_context(
    tree: &mut UiTree,
    node: NodeId,
    pointer: PointerEvent,
    byte: usize,
    allowed: impl Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    if tree.is_disabled(node) || !allowed(tree, node) {
        return RoutedOutcome::default();
    }
    route(
        tree,
        node,
        RoutedInput::TextContext { pointer, byte },
        &allowed,
    )
}

pub fn route_text_range(
    tree: &mut UiTree,
    node: NodeId,
    id: u64,
    range: std::ops::Range<usize>,
    pointer: PointerEvent,
    allowed: impl Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    if tree.is_disabled(node) || !allowed(tree, node) {
        return RoutedOutcome::default();
    }
    route(
        tree,
        node,
        RoutedInput::TextRangeActivated { id, range, pointer },
        &allowed,
    )
}

pub fn route_clipboard_cut(
    tree: &mut UiTree,
    node: NodeId,
    allowed: impl Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    if tree.is_disabled(node) || !allowed(tree, node) {
        return RoutedOutcome::default();
    }
    route(tree, node, RoutedInput::ClipboardCut, &allowed)
}

pub fn route_clipboard(
    tree: &mut UiTree,
    content: ClipboardPaste,
    allowed: impl Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    let Some(target) = tree
        .focused()
        .or_else(|| tree.root())
        .filter(|node| !tree.is_disabled(*node) && allowed(tree, *node))
    else {
        return RoutedOutcome::default();
    };
    route(tree, target, RoutedInput::ClipboardPaste(content), &allowed)
}

pub fn route_files(
    tree: &mut UiTree,
    paths: Vec<std::path::PathBuf>,
    at: Option<Point>,
    allowed: impl Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    if paths.is_empty() {
        return RoutedOutcome::default();
    }
    let at = at.or_else(|| tree.pointer_position());
    let target = at
        .and_then(|at| hit_test(tree, at))
        .or_else(|| tree.focused())
        .or_else(|| tree.root());
    let Some(target) = target.filter(|node| !tree.is_disabled(*node) && allowed(tree, *node))
    else {
        return RoutedOutcome::default();
    };
    route(
        tree,
        target,
        RoutedInput::FilesDropped {
            paths: paths.into(),
            at,
        },
        &allowed,
    )
}

pub fn route_external_file_drag(
    tree: &mut UiTree,
    event: ExternalFileDragEvent,
    allowed: impl Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    let pointer_at = |position| PointerEvent {
        contact: 0,
        kind: aqua_core::PointerKind::Mouse,
        position,
        buttons: 1,
    };
    let mut session = match &event {
        ExternalFileDragEvent::Enter { paths, position } => {
            if tree.input.drag.is_some() {
                return RoutedOutcome::default();
            }
            let paths: std::sync::Arc<[std::path::PathBuf]> = paths.clone().into();
            tree.input
                .external_file_drag
                .take()
                .unwrap_or_else(|| ExternalFileDragSession {
                    payload: DragPayload::new(paths.clone()),
                    paths,
                    pointer: pointer_at(*position),
                    target: None,
                })
        }
        _ => match tree.input.external_file_drag.take() {
            Some(session) => session,
            None => return RoutedOutcome::default(),
        },
    };
    let position = match &event {
        ExternalFileDragEvent::Enter { position, .. }
        | ExternalFileDragEvent::Move { position }
        | ExternalFileDragEvent::Submit { position } => Some(*position),
        ExternalFileDragEvent::Exit => None,
    };
    if let Some(position) = position {
        session.pointer = pointer_at(position);
        if !matches!(event, ExternalFileDragEvent::Submit { .. }) {
            route_input(tree, &InputEvent::PointerMove(session.pointer), &allowed);
        }
    }
    let target = position
        .and_then(|position| hit_test(tree, position))
        .filter(|node| !tree.is_disabled(*node) && allowed(tree, *node));
    if target != session.target {
        if let Some(previous) = session.target {
            route(
                tree,
                previous,
                RoutedInput::DragLeave {
                    payload: session.payload.clone(),
                    pointer: session.pointer,
                },
                &|_, _| true,
            );
        }
        if let Some(next) =
            target.filter(|_| !matches!(event, ExternalFileDragEvent::Submit { .. }))
        {
            route(
                tree,
                next,
                RoutedInput::DragEnter {
                    payload: session.payload.clone(),
                    pointer: session.pointer,
                },
                &allowed,
            );
        }
        session.target = target;
    }
    match event {
        ExternalFileDragEvent::Enter { .. } | ExternalFileDragEvent::Move { .. } => {
            let result = target
                .map(|node| {
                    route(
                        tree,
                        node,
                        RoutedInput::DragOver {
                            payload: session.payload.clone(),
                            pointer: session.pointer,
                        },
                        &allowed,
                    )
                })
                .unwrap_or_default();
            tree.input.external_file_drag = Some(session);
            result
        }
        ExternalFileDragEvent::Submit { position } => {
            let mut result = target
                .map(|node| {
                    route(
                        tree,
                        node,
                        RoutedInput::FilesDropped {
                            paths: session.paths.clone(),
                            at: Some(position),
                        },
                        &allowed,
                    )
                })
                .unwrap_or_default();
            if !result.handled && !result.default_prevented {
                let mut pointer = session.pointer;
                pointer.buttons = 0;
                result = route_input(
                    tree,
                    &InputEvent::PointerUp(pointer, aqua_core::PointerButton::Primary),
                    &allowed,
                );
            }
            if let Some(node) = target {
                route(
                    tree,
                    node,
                    RoutedInput::DragLeave {
                        payload: session.payload,
                        pointer: session.pointer,
                    },
                    &|_, _| true,
                );
            }
            result
        }
        ExternalFileDragEvent::Exit => RoutedOutcome::default(),
    }
}

pub fn route_input(
    tree: &mut UiTree,
    event: &InputEvent,
    allowed: impl Fn(&UiTree, NodeId) -> bool,
) -> RoutedOutcome {
    tree.observe_interaction_click(event);
    tree.queue_interaction_input(event);
    if matches!(event, InputEvent::Focused(false)) {
        tree.input.modifiers = Modifiers::NONE;
        tree.clear_presses();
        crate::controls::cancel_blocked(tree, |_, _| false);
        for node in tree.traverse() {
            let external = tree.get(node).is_some_and(|record| {
                matches!(
                    &record.kind, crate::tree::NodeKind::Text(text) if text.external_text_input
                )
            });
            let composing = !external
                && tree
                    .editor_snapshot(node)
                    .is_some_and(|editor| editor.composing);
            if composing {
                tree.edit_editor(node, |editor| {
                    editor.cancel_composition();
                });
            }
        }
    }
    if matches!(
        event,
        InputEvent::KeyDown(_) | InputEvent::KeyUp(_) | InputEvent::TextInput { .. }
    ) && crate::events::text_is_composing(tree)
        && !tree
            .focused()
            .and_then(|node| tree.get(node))
            .is_some_and(|record| {
                matches!(
                    &record.kind, crate::tree::NodeKind::Text(text) if text.external_text_input
                )
            })
    {
        return RoutedOutcome {
            handled: true,
            default_prevented: true,
            ..RoutedOutcome::default()
        };
    }
    let invalid: Vec<_> = tree
        .input
        .captures
        .iter()
        .filter(|(_, node)| {
            tree.get(**node).is_none() || tree.is_disabled(**node) || !allowed(tree, **node)
        })
        .map(|(contact, node)| (*contact, *node))
        .collect();
    for (contact, node) in invalid {
        route(
            tree,
            node,
            RoutedInput::Native(InputEvent::PointerCancel { contact }),
            &|_, _| true,
        );
        tree.input.captures.remove(&contact);
        tree.queue_interaction_event(
            node,
            crate::tree::interaction::InteractionEvent::Capture(false),
        );
    }
    if matches!(event, InputEvent::Focused(false)) {
        let captures: Vec<_> = tree
            .input
            .captures
            .iter()
            .map(|(contact, node)| (*contact, *node))
            .collect();
        for (contact, node) in captures {
            tree.queue_interaction_event(
                node,
                crate::tree::interaction::InteractionEvent::Capture(false),
            );
            route(
                tree,
                node,
                RoutedInput::Native(InputEvent::PointerCancel { contact }),
                &|_, _| true,
            );
        }
        tree.input.captures.clear();
        if let Some(pointer) = tree.input.last_pointer {
            for node in std::mem::take(&mut tree.input.hovered) {
                tree.queue_interaction_event(
                    node,
                    crate::tree::interaction::InteractionEvent::Hover(false),
                );
                route(tree, node, RoutedInput::PointerLeave(pointer), &|_, _| true);
            }
        }
    }
    if let InputEvent::PointerDown(pointer, _)
    | InputEvent::PointerUp(pointer, _)
    | InputEvent::PointerMove(pointer) = event
    {
        tree.input.last_pointer = Some(*pointer);
    }
    match event {
        InputEvent::ModifiersChanged(modifiers) => tree.input.modifiers = *modifiers,
        InputEvent::KeyDown(key) | InputEvent::KeyUp(key) => tree.input.modifiers = key.modifiers,
        _ => {}
    }
    if let InputEvent::PointerMove(pointer) = event {
        let hovered = hit_test(tree, pointer.position)
            .filter(|node| !tree.is_disabled(*node) && allowed(tree, *node))
            .map(|node| event_path(tree, node))
            .unwrap_or_default();
        let previous = std::mem::replace(&mut tree.input.hovered, hovered.clone());
        for node in previous.iter().filter(|node| !hovered.contains(node)) {
            tree.queue_interaction_event(
                *node,
                crate::tree::interaction::InteractionEvent::Hover(false),
            );
            route(tree, *node, RoutedInput::PointerLeave(*pointer), &|_, _| {
                true
            });
        }
        for node in hovered.iter().rev().filter(|node| !previous.contains(node)) {
            tree.queue_interaction_event(
                *node,
                crate::tree::interaction::InteractionEvent::Hover(true),
            );
            route(tree, *node, RoutedInput::PointerEnter(*pointer), &allowed);
        }
    }
    if tree.input.drag.as_ref().is_some_and(|drag| {
        tree.get(drag.source).is_none()
            || tree.is_disabled(drag.source)
            || !allowed(tree, drag.source)
    }) {
        finish_drag(tree, false);
    }
    match event {
        InputEvent::PointerMove(pointer)
            if tree
                .input
                .drag
                .as_ref()
                .is_some_and(|drag| drag.pointer.contact == pointer.contact) =>
        {
            if let Some(mut drag) = tree.input.drag.take() {
                move_drag(tree, &mut drag, *pointer, &allowed);
                tree.input.drag = Some(drag);
            }
            return RoutedOutcome {
                handled: true,
                default_prevented: true,
                ..RoutedOutcome::default()
            };
        }
        InputEvent::PointerUp(pointer, _)
            if tree
                .input
                .drag
                .as_ref()
                .is_some_and(|drag| drag.pointer.contact == pointer.contact) =>
        {
            if let Some(mut drag) = tree.input.drag.take() {
                move_drag(tree, &mut drag, *pointer, &allowed);
                let accepted = drag.accepted
                    && drag.target.is_some_and(|target| {
                        let dropped = route(
                            tree,
                            target,
                            RoutedInput::Drop {
                                payload: drag.payload.clone(),
                                pointer: *pointer,
                            },
                            &allowed,
                        );
                        dropped.handled || dropped.drop_accepted
                    });
                tree.input.drag = Some(drag);
                finish_drag(tree, accepted);
            }
            return RoutedOutcome {
                handled: true,
                default_prevented: true,
                ..RoutedOutcome::default()
            };
        }
        InputEvent::PointerCancel { contact }
            if tree
                .input
                .drag
                .as_ref()
                .is_some_and(|drag| drag.pointer.contact == *contact) =>
        {
            finish_drag(tree, false)
        }
        InputEvent::Focused(false) => finish_drag(tree, false),
        _ => {}
    }
    let contact = match event {
        InputEvent::PointerDown(pointer, _)
        | InputEvent::PointerUp(pointer, _)
        | InputEvent::PointerMove(pointer) => Some(pointer.contact),
        InputEvent::PointerCancel { contact } => Some(*contact),
        _ => None,
    };
    let captured = contact.and_then(|contact| tree.pointer_capture(contact));
    let target = captured.or_else(|| match event {
        InputEvent::PointerDown(pointer, _)
        | InputEvent::PointerUp(pointer, _)
        | InputEvent::PointerMove(pointer) => hit_test(tree, pointer.position),
        InputEvent::Scroll { at, .. } => hit_test(tree, *at),
        _ => tree.focused().or_else(|| tree.root()),
    });
    let mut outcome = target
        .filter(|node| !tree.is_disabled(*node) && allowed(tree, *node))
        .map(|node| route(tree, node, RoutedInput::Native(event.clone()), &allowed))
        .unwrap_or_default();
    if !outcome.default_prevented {
        if let InputEvent::KeyDown(key) = event {
            if !crate::events::text_is_composing(tree) {
                if let Some(target) = target {
                    let shortcut = event_path(tree, target)
                        .into_iter()
                        .filter(|node| allowed(tree, *node) && !tree.is_disabled(*node))
                        .find_map(|node| {
                            tree.input
                                .shortcuts
                                .get(&node)?
                                .iter()
                                .find(|binding| {
                                    keys_match(binding.key, key.key)
                                        && binding.modifiers == key.modifiers
                                        && (!key.repeat || binding.repeat)
                                })
                                .copied()
                                .map(|binding| (node, binding))
                        });
                    if let Some((node, shortcut)) = shortcut {
                        if dispatch_action_allowed(
                            tree,
                            node,
                            shortcut.action,
                            Some(crate::tree::ActivationSource::Keyboard(*key)),
                            &allowed,
                        ) {
                            outcome.handled = true;
                            outcome.default_prevented = true;
                        }
                    }
                }
            }
        }
    }
    match event {
        InputEvent::PointerUp(_, _) | InputEvent::PointerCancel { .. } => {
            if let Some(contact) = contact {
                if let Some(node) = tree.input.captures.remove(&contact) {
                    tree.queue_interaction_event(
                        node,
                        crate::tree::interaction::InteractionEvent::Capture(false),
                    );
                }
            }
        }
        InputEvent::Focused(false) => {
            tree.input.captures.clear();
            tree.input.hovered.clear();
            tree.input.modifiers = Modifiers::NONE;
        }
        _ => {}
    }
    outcome
}
