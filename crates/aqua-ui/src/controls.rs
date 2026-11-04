use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, PointerButton, Role,
};

use crate::events::hit_test;
use crate::input::visual_bounds;
use crate::style::{Axis, Length};
use crate::tree::{ActionEvent, ActionKind, UiTree};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl SliderRange {
    pub fn new(min: f64, max: f64, step: f64) -> Self {
        let (min, max) = match min.is_finite() && max.is_finite() && (max - min).is_finite() {
            true => (min.min(max), min.max(max)),
            false => (0.0, 1.0),
        };
        let step = match step.is_finite() && step > 0.0 {
            true => step,
            false => ((max - min) / 100.0).max(f64::MIN_POSITIVE),
        };
        Self { min, max, step }
    }

    pub fn snap(self, value: f64) -> f64 {
        let value = match value.is_finite() {
            true => value.clamp(self.min, self.max),
            false => self.min,
        };
        if value == self.min || value == self.max {
            return value;
        }
        ((value - self.min) / self.step)
            .round()
            .mul_add(self.step, self.min)
            .clamp(self.min, self.max)
    }

    pub fn fraction(self, value: f64) -> f32 {
        match self.max > self.min {
            true => ((self.snap(value) - self.min) / (self.max - self.min)) as f32,
            false => 0.0,
        }
    }
}

impl Default for SliderRange {
    fn default() -> Self {
        Self::new(0.0, 1.0, 0.01)
    }
}

#[derive(Clone)]
pub struct SliderControl {
    pub range: SliderRange,
    pub value: f64,
    pub on_change: Option<Rc<dyn Fn(f64)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub disabled: bool,
}

impl SelectOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Clone)]
pub struct SelectControl {
    pub options: Vec<SelectOption>,
    pub selected: Option<usize>,
    pub on_select: Option<Rc<dyn Fn(usize)>>,
    pub(crate) open: bool,
    pub(crate) highlighted: Option<usize>,
    prefix: String,
    typed_at: f64,
}

impl SelectControl {
    pub fn new(
        options: Vec<SelectOption>,
        selected: Option<usize>,
        on_select: Option<Rc<dyn Fn(usize)>>,
    ) -> Self {
        let selected =
            selected.filter(|index| options.get(*index).is_some_and(|option| !option.disabled));
        Self {
            options,
            selected,
            on_select,
            open: false,
            highlighted: selected,
            prefix: String::new(),
            typed_at: 0.0,
        }
    }

    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn next(&self, from: Option<usize>, forward: bool) -> Option<usize> {
        let len = self.options.len();
        let start = from.unwrap_or(if forward { len.saturating_sub(1) } else { 0 });
        (1..=len)
            .map(|offset| {
                if forward {
                    (start + offset) % len
                } else {
                    (start + len - offset % len) % len
                }
            })
            .find(|index| !self.options[*index].disabled)
    }
}

#[derive(Clone)]
pub struct TabsControl {
    pub selected: usize,
    pub count: usize,
    pub on_select: Option<Rc<dyn Fn(usize)>>,
}

#[derive(Clone)]
pub struct ResizeControl {
    pub axis: Axis,
    pub range: SliderRange,
    pub value: f64,
    pub on_resize: Option<Rc<dyn Fn(f64)>>,
}

#[derive(Clone)]
pub enum Control {
    Slider(SliderControl),
    Select(SelectControl),
    SelectPopup(NodeId),
    Tabs(TabsControl),
    Resize(ResizeControl),
}

impl fmt::Debug for Control {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Slider(control) => formatter
                .debug_tuple("Slider")
                .field(&control.value)
                .finish(),
            Self::Select(control) => formatter
                .debug_tuple("Select")
                .field(&control.selected)
                .finish(),
            Self::SelectPopup(owner) => formatter.debug_tuple("SelectPopup").field(owner).finish(),
            Self::Tabs(control) => formatter
                .debug_tuple("Tabs")
                .field(&control.selected)
                .finish(),
            Self::Resize(control) => formatter
                .debug_tuple("Resize")
                .field(&control.value)
                .finish(),
        }
    }
}

impl Control {
    pub(crate) fn preserve(&mut self, previous: &Self) {
        if let (Self::Select(next), Self::Select(previous)) = (self, previous) {
            if next.options == previous.options {
                next.open = previous.open;
                next.highlighted = previous.highlighted;
                next.prefix.clone_from(&previous.prefix);
                next.typed_at = previous.typed_at;
            }
        }
    }
}

#[derive(Debug)]
pub enum ControlRequest {
    OpenPopup {
        owner: NodeId,
        content: Box<crate::element::Element>,
        options: crate::popup::PopupOptions,
    },
    ArmTooltip {
        owner: NodeId,
        tooltip: crate::popup::Tooltip,
    },
    HideTooltip {
        owner: NodeId,
    },
    OpenSelect {
        owner: NodeId,
    },
    CloseSelect {
        owner: NodeId,
    },
    HighlightSelect {
        owner: NodeId,
        index: usize,
    },
}

#[derive(Clone, Copy)]
struct Capture {
    node: NodeId,
    down: aqua_core::PointerEvent,
    origin: Point,
    value: f64,
}

#[derive(Default)]
pub(crate) struct ControlRouter {
    captures: HashMap<u64, Capture>,
    requests: Vec<ControlRequest>,
    clock: f64,
}

impl ControlRouter {
    pub(crate) fn request(&mut self, request: ControlRequest) {
        self.requests.push(request);
    }

    pub(crate) fn remove_node(&mut self, node: NodeId) {
        self.captures.retain(|_, capture| capture.node != node);
        self.requests.retain(|request| match request {
            ControlRequest::OpenSelect { owner }
            | ControlRequest::CloseSelect { owner }
            | ControlRequest::HighlightSelect { owner, .. }
            | ControlRequest::OpenPopup { owner, .. }
            | ControlRequest::ArmTooltip { owner, .. }
            | ControlRequest::HideTooltip { owner } => *owner != node,
        });
    }
}

impl UiTree {
    pub fn advance_control_clock(&mut self, now: f64) {
        if now.is_finite() && now > self.control_router.clock {
            self.control_router.clock = now;
        }
    }

    pub(crate) fn control_clock(&self) -> f64 {
        self.control_router.clock
    }

    pub fn take_control_requests(&mut self) -> Vec<ControlRequest> {
        std::mem::take(&mut self.control_router.requests)
    }

    pub fn control(&self, node: NodeId) -> Option<&Control> {
        self.get(node)?.control.as_deref()
    }

    pub fn select_options(&self, node: NodeId) -> Option<&SelectControl> {
        match self.control(node)? {
            Control::Select(control) => Some(control),
            _ => None,
        }
    }

    pub fn set_control_value(&mut self, node: NodeId, value: f64) -> bool {
        if self.is_disabled(node) {
            return false;
        }
        let Some(mut control) = self.get_mut(node).and_then(|record| record.control.take()) else {
            return false;
        };
        let (changed, callback, fraction) = match control.as_mut() {
            Control::Slider(slider) => {
                let value = slider.range.snap(value);
                let changed = slider.value != value;
                slider.value = value;
                (
                    changed,
                    slider.on_change.clone(),
                    Some(slider.range.fraction(value)),
                )
            }
            Control::Resize(resize) => {
                let value = resize.range.snap(value);
                let changed = resize.value != value;
                resize.value = value;
                (changed, resize.on_resize.clone(), None)
            }
            _ => {
                self.get_mut(node).unwrap().control = Some(control);
                return false;
            }
        };
        let value = match control.as_ref() {
            Control::Slider(slider) => slider.value,
            Control::Resize(resize) => resize.value,
            _ => value,
        };
        self.get_mut(node).unwrap().control = Some(control);
        if changed {
            if let Some(fraction) = fraction {
                if let Some(fill) = self
                    .get(node)
                    .and_then(|record| record.children.first())
                    .copied()
                {
                    let mut style = self.get(fill).unwrap().style.clone();
                    style.width = Length::Pct(fraction * 100.0);
                    self.set_style(fill, style);
                }
            }
            let mut semantics = self.get(node).unwrap().semantics.clone();
            semantics.value_text = Some(value.to_string());
            self.set_semantics(node, semantics);
            if let Some(callback) = callback {
                callback(value);
            }
        }
        changed
    }

    pub fn choose_control_option(&mut self, owner: NodeId, index: usize) -> bool {
        if self.is_disabled(owner) {
            return false;
        }
        let Some(mut control) = self.get_mut(owner).and_then(|record| record.control.take()) else {
            return false;
        };
        let Control::Select(select) = control.as_mut() else {
            self.get_mut(owner).unwrap().control = Some(control);
            return false;
        };
        let Some(option) = select.options.get(index).filter(|option| !option.disabled) else {
            self.get_mut(owner).unwrap().control = Some(control);
            return false;
        };
        let label = option.label.clone();
        let changed = select.selected != Some(index);
        select.selected = Some(index);
        select.highlighted = Some(index);
        let close = select.open;
        select.open = false;
        let callback = select.on_select.clone();
        self.get_mut(owner).unwrap().control = Some(control);
        if let Some(label_node) = self
            .get(owner)
            .and_then(|record| record.children.first())
            .copied()
        {
            self.set_text(label_node, label.clone());
        }
        let mut semantics = self.get(owner).unwrap().semantics.clone();
        semantics.value_text = Some(label);
        self.set_semantics(owner, semantics);
        if close {
            self.control_router
                .requests
                .push(ControlRequest::CloseSelect { owner });
        }
        if changed {
            if let Some(callback) = callback {
                callback(index);
            }
        }
        true
    }

    pub fn set_control_expanded(&mut self, owner: NodeId, expanded: bool) -> bool {
        if self.is_disabled(owner) {
            return false;
        }
        let Some(Control::Select(control)) = self.control(owner) else {
            return false;
        };
        if control.open == expanded {
            return false;
        }
        let Some(Control::Select(control)) = self
            .get_mut(owner)
            .and_then(|record| record.control.as_deref_mut())
        else {
            return false;
        };
        control.open = expanded;
        if expanded {
            control.highlighted = control.selected.or_else(|| control.next(None, true));
            self.control_router
                .requests
                .push(ControlRequest::OpenSelect { owner });
        } else {
            self.control_router
                .requests
                .push(ControlRequest::CloseSelect { owner });
        }
        true
    }

    pub fn close_control_select(&mut self, owner: NodeId) {
        if let Some(record) = self.get_mut(owner) {
            if let Some(Control::Select(control)) = record.control.as_deref_mut() {
                control.open = false;
                control.prefix.clear();
            }
        }
    }
}

fn owner(tree: &UiTree, mut node: NodeId) -> Option<NodeId> {
    loop {
        let record = tree.get(node)?;
        match record.control.as_deref() {
            Some(Control::SelectPopup(owner)) => return tree.get(*owner).map(|_| *owner),
            Some(_) => return Some(node),
            None => node = record.parent?,
        }
    }
}

fn pointer_owner(tree: &UiTree, mut node: NodeId) -> Option<NodeId> {
    loop {
        let record = tree.get(node)?;
        match record.control.as_deref() {
            Some(Control::SelectPopup(_)) => return None,
            Some(_) => return Some(node),
            None => node = record.parent?,
        }
    }
}

fn popup_option(tree: &UiTree, mut node: NodeId) -> Option<(NodeId, usize)> {
    loop {
        let parent = tree.get(node)?.parent?;
        let record = tree.get(parent)?;
        if let Some(Control::SelectPopup(owner)) = record.control.as_deref() {
            return Some((
                *owner,
                record.children.iter().position(|child| *child == node)?,
            ));
        }
        node = parent;
    }
}

fn seek(tree: &mut UiTree, node: NodeId, pointer: Point, capture: Capture) {
    let value = match tree.control(node) {
        Some(Control::Slider(slider)) => visual_bounds(tree, node).map(|rect| {
            slider.range.min
                + (slider.range.max - slider.range.min)
                    * ((pointer.x - rect.x) / rect.w.max(1.0)).clamp(0.0, 1.0) as f64
        }),
        Some(Control::Resize(resize)) => Some(
            capture.value
                + match resize.axis {
                    Axis::Row => (pointer.x - capture.origin.x) as f64,
                    Axis::Column => (pointer.y - capture.origin.y) as f64,
                } / tree
                    .visual_transform(node)
                    .map_or(1.0, |transform| transform.a as f64),
        ),
        _ => None,
    };
    if let Some(value) = value {
        tree.set_control_value(node, value);
    }
}

fn select_key(tree: &mut UiTree, owner: NodeId, key: &KeyEvent) -> bool {
    if key.modifiers.contains(Modifiers::CONTROL) || key.modifiers.contains(Modifiers::LOGO) {
        return false;
    }
    let now = tree.control_router.clock;
    let focused_option = tree
        .focused()
        .and_then(|node| popup_option(tree, node))
        .filter(|(candidate, _)| *candidate == owner)
        .map(|(_, index)| index);
    let Some(mut control) = tree.get_mut(owner).and_then(|record| record.control.take()) else {
        return false;
    };
    let Control::Select(select) = control.as_mut() else {
        tree.get_mut(owner).unwrap().control = Some(control);
        return false;
    };
    if select.open {
        select.highlighted = focused_option.or(select.highlighted);
    }
    let mut choose = None;
    let consumed = match key.key {
        Key::Named(NamedKey::Enter) | Key::Char(' ') => {
            if select.open {
                choose = select.highlighted;
            } else {
                select.open = true;
                select.highlighted = select.selected.or_else(|| select.next(None, true));
                tree.control_router
                    .requests
                    .push(ControlRequest::OpenSelect { owner });
            }
            true
        }
        Key::Named(NamedKey::Escape) if select.open => {
            select.open = false;
            tree.control_router
                .requests
                .push(ControlRequest::CloseSelect { owner });
            true
        }
        Key::Named(NamedKey::Down | NamedKey::Up | NamedKey::Home | NamedKey::End) => {
            let next = match key.key {
                Key::Named(NamedKey::Home) => {
                    select.options.iter().position(|option| !option.disabled)
                }
                Key::Named(NamedKey::End) => {
                    select.options.iter().rposition(|option| !option.disabled)
                }
                _ => select.next(
                    if select.open {
                        select.highlighted
                    } else {
                        select.selected
                    },
                    key.key == Key::Named(NamedKey::Down),
                ),
            };
            if select.open {
                select.highlighted = next;
                if let Some(index) = next {
                    tree.control_router
                        .requests
                        .push(ControlRequest::HighlightSelect { owner, index });
                }
            } else {
                choose = next;
            }
            true
        }
        Key::Char(character)
            if !key.modifiers.contains(Modifiers::ALT) && !character.is_control() =>
        {
            if now - select.typed_at > 0.7 {
                select.prefix.clear();
            }
            select.typed_at = now;
            let character: String = character.to_lowercase().collect();
            let repeated = select.prefix == character;
            if repeated {
                select.prefix.clear();
            }
            select.prefix.push_str(&character);
            let from = if select.open {
                select.highlighted
            } else {
                select.selected
            };
            let len = select.options.len();
            let next = (1..=len)
                .map(|offset| (from.unwrap_or(len.saturating_sub(1)) + offset) % len)
                .find(|index| {
                    !select.options[*index].disabled
                        && select.options[*index]
                            .label
                            .to_lowercase()
                            .starts_with(&select.prefix)
                });
            if select.open {
                select.highlighted = next.or(select.highlighted);
                if let Some(index) = next {
                    tree.control_router
                        .requests
                        .push(ControlRequest::HighlightSelect { owner, index });
                }
            } else {
                choose = next;
            }
            true
        }
        _ => false,
    };
    tree.get_mut(owner).unwrap().control = Some(control);
    if let Some(index) = choose {
        tree.choose_control_option(owner, index);
    }
    consumed
}

fn tabs_key(tree: &mut UiTree, owner: NodeId, key: &KeyEvent) -> bool {
    if !key.modifiers.is_empty() {
        return false;
    }
    let Some(Control::Tabs(tabs)) = tree.control(owner) else {
        return false;
    };
    let count = tabs.count.min(tree.get(owner).unwrap().children.len());
    if count == 0 {
        return false;
    }
    let focused = tree
        .focused()
        .and_then(|focused| {
            tree.get(owner).unwrap().children[..count]
                .iter()
                .position(|node| *node == focused)
        })
        .unwrap_or(tabs.selected.min(count - 1));
    let next = match key.key {
        Key::Named(NamedKey::Right | NamedKey::Down) => (focused + 1) % count,
        Key::Named(NamedKey::Left | NamedKey::Up) => (focused + count - 1) % count,
        Key::Named(NamedKey::Home) => 0,
        Key::Named(NamedKey::End) => count - 1,
        _ => return false,
    };
    let callback = tabs.on_select.clone();
    let children = tree.get(owner).unwrap().children[..count].to_vec();
    for (index, node) in children.iter().copied().enumerate() {
        if let Some(record) = tree.get_mut(node) {
            record.focusable = true;
            record.tab_index = if index == next { 0 } else { -1 };
            record.semantics.selected = index == next;
            record.semantics.role = Role::Tab;
        }
    }
    if let Some(Control::Tabs(tabs)) = tree
        .get_mut(owner)
        .and_then(|record| record.control.as_deref_mut())
    {
        tabs.selected = next;
    }
    tree.set_focused(Some(children[next]));
    if let Some(callback) = callback {
        callback(next);
    }
    true
}

pub fn dispatch(tree: &mut UiTree, event: &InputEvent) -> bool {
    match event {
        InputEvent::PointerDown(pointer, PointerButton::Primary) => {
            let Some(node) = hit_test(tree, pointer.position)
                .and_then(|hit| pointer_owner(tree, hit))
                .filter(|node| !tree.is_disabled(*node))
            else {
                return false;
            };
            let value = match tree.control(node) {
                Some(Control::Slider(control)) => control.value,
                Some(Control::Resize(control)) => control.value,
                Some(Control::Select(_)) => 0.0,
                _ => return false,
            };
            tree.set_focused(Some(node));
            let capture = Capture {
                node,
                down: *pointer,
                origin: pointer.position,
                value,
            };
            tree.control_router
                .captures
                .insert(pointer.contact, capture);
            seek(tree, node, pointer.position, capture);
            true
        }
        InputEvent::PointerMove(pointer) => {
            let Some(capture) = tree.control_router.captures.get(&pointer.contact).copied() else {
                if let Some((owner, index)) =
                    hit_test(tree, pointer.position).and_then(|hit| popup_option(tree, hit))
                {
                    let enabled = tree
                        .select_options(owner)
                        .and_then(|select| select.options.get(index))
                        .is_some_and(|option| !option.disabled);
                    if enabled {
                        if let Some(Control::Select(select)) = tree
                            .get_mut(owner)
                            .and_then(|record| record.control.as_deref_mut())
                        {
                            select.highlighted = Some(index);
                        }
                        tree.control_router
                            .requests
                            .push(ControlRequest::HighlightSelect { owner, index });
                    }
                }
                return false;
            };
            if tree.get(capture.node).is_none() || tree.is_disabled(capture.node) {
                tree.control_router.captures.remove(&pointer.contact);
                return false;
            }
            seek(tree, capture.node, pointer.position, capture);
            true
        }
        InputEvent::PointerUp(pointer, PointerButton::Primary) => {
            let Some(capture) = tree.control_router.captures.remove(&pointer.contact) else {
                return false;
            };
            if tree.get(capture.node).is_none() || tree.is_disabled(capture.node) {
                return true;
            }
            seek(tree, capture.node, pointer.position, capture);
            if matches!(tree.control(capture.node), Some(Control::Select(_)))
                && (pointer.kind != aqua_core::PointerKind::Touch
                    || tree.interaction_tap_count(pointer.contact).is_some())
                && hit_test(tree, pointer.position).and_then(|hit| pointer_owner(tree, hit))
                    == Some(capture.node)
            {
                select_key(
                    tree,
                    capture.node,
                    &KeyEvent {
                        key: Key::Named(NamedKey::Enter),
                        modifiers: Modifiers::NONE,
                        repeat: false,
                    },
                );
                tree.fire_action(ActionEvent {
                    source: if pointer.kind == aqua_core::PointerKind::Touch {
                        tree.interaction_tap_count(pointer.contact)
                            .map(|tap_count| crate::tree::ActivationSource::Touch {
                                position: pointer.position,
                                tap_count,
                                long_press: false,
                            })
                    } else {
                        Some(crate::tree::ActivationSource::Pointer {
                            down: capture.down,
                            up: *pointer,
                            button: PointerButton::Primary,
                            modifiers: tree.input.modifiers(),
                            click_count: tree
                                .interaction_click_count(pointer, PointerButton::Primary),
                        })
                    },
                    node: capture.node,
                    kind: ActionKind::Activate,
                    text: None,
                    focused: None,
                });
            }
            true
        }
        InputEvent::PointerCancel { contact } => {
            tree.control_router.captures.remove(contact).is_some()
        }
        InputEvent::Focused(false) => {
            tree.control_router.captures.clear();
            false
        }
        InputEvent::KeyDown(key) => {
            let Some(node) = tree
                .focused()
                .and_then(|focused| owner(tree, focused))
                .filter(|node| !tree.is_disabled(*node))
            else {
                return false;
            };
            match tree.control(node) {
                Some(Control::Select(_)) => select_key(tree, node, key),
                Some(Control::Tabs(_)) => tabs_key(tree, node, key),
                Some(Control::Slider(slider)) => {
                    if key.modifiers.contains(Modifiers::CONTROL)
                        || key.modifiers.contains(Modifiers::ALT)
                        || key.modifiers.contains(Modifiers::LOGO)
                    {
                        return false;
                    }
                    let value = match key.key {
                        Key::Named(NamedKey::Left | NamedKey::Down) => {
                            slider.value - slider.range.step
                        }
                        Key::Named(NamedKey::Right | NamedKey::Up) => {
                            slider.value + slider.range.step
                        }
                        Key::Named(NamedKey::PageDown) => slider.value - slider.range.step * 10.0,
                        Key::Named(NamedKey::PageUp) => slider.value + slider.range.step * 10.0,
                        Key::Named(NamedKey::Home) => slider.range.min,
                        Key::Named(NamedKey::End) => slider.range.max,
                        _ => return false,
                    };
                    tree.set_control_value(node, value);
                    true
                }
                Some(Control::Resize(resize)) => {
                    let amount = if key.modifiers.contains(Modifiers::SHIFT) {
                        10.0
                    } else {
                        1.0
                    };
                    let value = match key.key {
                        Key::Named(NamedKey::Left | NamedKey::Up) => resize.value - amount,
                        Key::Named(NamedKey::Right | NamedKey::Down) => resize.value + amount,
                        Key::Named(NamedKey::Home) => resize.range.min,
                        Key::Named(NamedKey::End) => resize.range.max,
                        _ => return false,
                    };
                    tree.set_control_value(node, value);
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

pub struct ResizeHandle {
    style: crate::style::Style,
    control: ResizeControl,
}

impl ResizeHandle {
    pub fn new(axis: Axis, value: f64, min: f64, max: f64) -> Self {
        Self {
            style: match axis {
                Axis::Row => crate::style::Style::new()
                    .width(crate::style::px(6.0))
                    .height(Length::Pct(100.0)),
                Axis::Column => crate::style::Style::new()
                    .width(Length::Pct(100.0))
                    .height(crate::style::px(6.0)),
            },
            control: ResizeControl {
                axis,
                range: SliderRange::new(min, max, 1.0),
                value,
                on_resize: None,
            },
        }
    }

    pub fn style(mut self, style: crate::style::Style) -> Self {
        self.style = style;
        self
    }

    pub fn on_resize(mut self, on_resize: impl Fn(f64) + 'static) -> Self {
        self.control.on_resize = Some(Rc::new(on_resize));
        self
    }

    pub fn build(self) -> crate::element::Element {
        let cursor = match self.control.axis {
            Axis::Row => aqua_core::CursorIcon::ColResize,
            Axis::Column => aqua_core::CursorIcon::RowResize,
        };
        crate::element::Box::new()
            .style(self.style)
            .control(Control::Resize(self.control))
            .build()
            .role(Role::Separator)
            .cursor(cursor)
    }
}

pub fn cancel_blocked(tree: &mut UiTree, allowed: impl Fn(&UiTree, NodeId) -> bool) {
    let blocked: Vec<_> = tree
        .control_router
        .captures
        .iter()
        .filter(|(_, capture)| {
            tree.get(capture.node).is_none()
                || tree.is_disabled(capture.node)
                || !allowed(tree, capture.node)
        })
        .map(|(contact, _)| *contact)
        .collect();
    for contact in blocked {
        tree.control_router.captures.remove(&contact);
    }
}

pub(crate) fn initialize_node(tree: &mut UiTree, node: NodeId) {
    let Some(control) = tree.control(node) else {
        return;
    };
    let (value, tabs) = match control {
        Control::Slider(slider) => (Some(slider.value.to_string()), None),
        Control::Resize(resize) => (Some(resize.value.to_string()), None),
        Control::Select(select) => (
            select
                .selected
                .and_then(|index| select.options.get(index))
                .map(|option| option.label.clone()),
            None,
        ),
        Control::Tabs(tabs) => (None, Some((tabs.count, tabs.selected))),
        Control::SelectPopup(_) => (None, None),
    };
    if let Some(value) = value {
        if tree
            .get(node)
            .is_some_and(|record| record.semantics.value_text.as_ref() != Some(&value))
        {
            let mut semantics = tree.get(node).unwrap().semantics.clone();
            semantics.value_text = Some(value);
            tree.set_semantics(node, semantics);
        }
    }
    if let Some((count, selected)) = tabs {
        let children = tree
            .get(node)
            .unwrap()
            .children
            .iter()
            .take(count)
            .copied()
            .collect::<Vec<_>>();
        for (index, child) in children.into_iter().enumerate() {
            let wanted_index = if index == selected { 0 } else { -1 };
            let changed = tree.get(child).is_some_and(|record| {
                record.tab_index != wanted_index
                    || !record.focusable
                    || record.semantics.role != Role::Tab
                    || record.semantics.selected != (index == selected)
            });
            if changed {
                if let Some(record) = tree.get_mut(child) {
                    record.focusable = true;
                    record.tab_index = wanted_index;
                    record.semantics.role = Role::Tab;
                    record.semantics.selected = index == selected;
                }
            }
        }
    }
}

pub fn pointer_capture(tree: &UiTree, contact: u64) -> Option<NodeId> {
    tree.control_router
        .captures
        .get(&contact)
        .map(|capture| capture.node)
}
pub fn captured_node(tree: &UiTree) -> Option<NodeId> {
    tree.control_router
        .captures
        .values()
        .next()
        .map(|capture| capture.node)
}
