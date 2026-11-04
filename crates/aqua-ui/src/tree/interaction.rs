use crate::style::{LayoutStyle, Paint, Style};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InputModality {
    #[default]
    Pointer,
    Keyboard,
    Touch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionEvent {
    Hover(bool),
    Focus(bool),
    Press(bool),
    Disabled(bool),
    Dragging(bool),
    Capture(bool),
    Modality(InputModality),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InteractionState {
    pub hovered: bool,
    pub focused: bool,
    pub pressed: bool,
    pub disabled: bool,
    pub dragging: bool,
    pub captured: bool,
    pub modality: InputModality,
}

impl InteractionState {
    pub fn transition(&mut self, event: InteractionEvent) -> bool {
        let previous = *self;
        match event {
            InteractionEvent::Hover(hovered) => self.hovered = hovered,
            InteractionEvent::Focus(focused) => self.focused = focused,
            InteractionEvent::Press(pressed) => self.pressed = pressed,
            InteractionEvent::Disabled(disabled) => {
                self.disabled = disabled;
                if disabled {
                    self.pressed = false;
                }
            }
            InteractionEvent::Dragging(dragging) => self.dragging = dragging,
            InteractionEvent::Capture(captured) => self.captured = captured,
            InteractionEvent::Modality(modality) => self.modality = modality,
        }
        *self != previous
    }

    pub fn focus_visible(self) -> bool {
        !self.disabled && self.focused && self.modality == InputModality::Keyboard
    }

    pub fn hover_visible(self) -> bool {
        !self.disabled
            && !self.dragging
            && self.modality != InputModality::Touch
            && (self.captured || (self.hovered && self.modality == InputModality::Pointer))
    }

    pub fn press_visible(self) -> bool {
        !self.disabled && self.pressed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionStyleKind {
    Hover,
    FocusVisible,
    Pressed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InteractionStyles {
    pub hover: Option<Style>,
    pub focus_visible: Option<Style>,
    pub pressed: Option<Style>,
}

impl InteractionStyles {
    pub fn set(&mut self, kind: InteractionStyleKind, style: Style) {
        match kind {
            InteractionStyleKind::Hover => self.hover = Some(style),
            InteractionStyleKind::FocusVisible => self.focus_visible = Some(style),
            InteractionStyleKind::Pressed => self.pressed = Some(style),
        }
    }

    pub fn refinement(&self, state: InteractionState) -> Style {
        let mut style = Style::new();
        if state.focus_visible() {
            if let Some(focus) = &self.focus_visible {
                style.merge(focus);
            }
        }
        if state.hover_visible() {
            if let Some(hover) = &self.hover {
                style.merge(hover);
            }
        }
        if state.press_visible() {
            if let Some(pressed) = &self.pressed {
                style.merge(pressed);
            }
        }
        style
    }

    pub fn resolve(
        &self,
        state: InteractionState,
        base_layout: &LayoutStyle,
        base_paint: &Paint,
    ) -> (LayoutStyle, Paint) {
        let mut layout = base_layout.clone();
        let mut paint = base_paint.clone();
        self.refinement(state).apply_to(&mut layout, &mut paint);
        (layout, paint)
    }
}

impl super::UiTree {
    pub fn apply_interaction_style(
        &mut self,
        node: aqua_core::NodeId,
        styles: &InteractionStyles,
        state: InteractionState,
        base_layout: &LayoutStyle,
        base_paint: &Paint,
    ) -> bool {
        let (layout, paint) = styles.resolve(state, base_layout, base_paint);
        let layout_changed = self.get(node).is_some_and(|record| record.style != layout)
            && self.set_style(node, layout);
        let paint_changed = self.set_paint(node, paint);
        layout_changed || paint_changed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionBoolProperty {
    Expanded,
    Selected,
    Checked,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InteractionSemantic {
    Expanded(Option<bool>),
    Level(Option<usize>),
    Selected(bool),
    Checked(Option<bool>),
    Value(Option<String>),
}

#[derive(Debug)]
pub enum InteractionCommand {
    Bind {
        node: aqua_core::NodeId,
        layout: Box<LayoutStyle>,
        paint: Box<Paint>,
        styled: bool,
    },
    Style {
        node: aqua_core::NodeId,
        kind: InteractionStyleKind,
        style: Box<Style>,
    },
    Event {
        node: aqua_core::NodeId,
        event: InteractionEvent,
    },
    Modality(InputModality),
    TypographyColor {
        node: aqua_core::NodeId,
        own: Option<aqua_core::Oklch>,
        own_gpui: Option<Option<aqua_core::GpuiHsla>>,
        own_source: Option<Option<aqua_core::GpuiTextColor>>,
    },
    Semantics {
        node: aqua_core::NodeId,
        value: InteractionSemantic,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InteractionSemantics {
    pub expanded: Option<bool>,
    pub level: Option<usize>,
    pub checked: Option<bool>,
}

#[derive(Clone, Debug)]
struct InteractionBase {
    styles: InteractionStyles,
    layout: LayoutStyle,
    paint: Paint,
}

#[derive(Clone, Debug)]
struct RetainedInteraction {
    state: InteractionState,
    base: Option<Box<InteractionBase>>,
    semantics: InteractionSemantics,
    color_registered: bool,
    own_color: Option<aqua_core::Oklch>,
    own_gpui_color: Option<Option<aqua_core::GpuiHsla>>,
    own_source_color: Option<Option<aqua_core::GpuiTextColor>>,
}

impl RetainedInteraction {
    fn refinement(&self) -> Style {
        self.base
            .as_ref()
            .map_or_else(Style::new, |base| base.styles.refinement(self.state))
    }
}

#[derive(Debug, Default)]
pub struct InteractionStore {
    nodes: std::collections::HashMap<aqua_core::NodeId, RetainedInteraction>,
    modality: InputModality,
    click_settings: aqua_core::ClickSettings,
    last_click: Option<PointerClick>,
    pending_touch: Option<PendingTouch>,
    last_tap: Option<CompletedTap>,
    emitted_tap: Option<(u64, usize)>,
}

#[derive(Debug)]
struct PointerClick {
    at: aqua_core::Point,
    button: aqua_core::PointerButton,
    contact: u64,
    time: f64,
    count: usize,
}

#[derive(Debug)]
struct PendingTouch {
    contact: u64,
    start: aqua_core::Point,
    panning: bool,
}

#[derive(Debug)]
struct CompletedTap {
    position: aqua_core::Point,
    time: f64,
    count: usize,
}

fn distance_squared(a: aqua_core::Point, b: aqua_core::Point) -> f32 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
}

impl super::CommandSink {
    pub fn interaction(&self, command: InteractionCommand) {
        self.push(super::UiCommand::Interaction(Box::new(command)));
    }
}

impl super::UiTree {
    pub fn set_interaction_click_settings(&mut self, settings: aqua_core::ClickSettings) {
        if self.interaction.click_settings != settings {
            self.interaction.click_settings = settings;
            self.interaction.last_click = None;
        }
    }

    pub(crate) fn observe_interaction_click(&mut self, event: &aqua_core::InputEvent) {
        use aqua_core::{InputEvent, PointerKind};
        match event {
            InputEvent::PointerDown(pointer, _) if pointer.kind == PointerKind::Touch => {
                self.interaction.emitted_tap = None;
                if self.interaction.pending_touch.is_none() {
                    self.interaction.pending_touch = Some(PendingTouch {
                        contact: pointer.contact,
                        start: pointer.position,
                        panning: false,
                    });
                }
            }
            InputEvent::PointerMove(pointer) if pointer.kind == PointerKind::Touch => {
                if let Some(touch) = self
                    .interaction
                    .pending_touch
                    .as_mut()
                    .filter(|touch| touch.contact == pointer.contact)
                {
                    touch.panning |= distance_squared(touch.start, pointer.position) > 64.0;
                }
            }
            InputEvent::PointerUp(pointer, _) if pointer.kind == PointerKind::Touch => {
                self.interaction.emitted_tap = None;
                if self
                    .interaction
                    .pending_touch
                    .take_if(|touch| touch.contact == pointer.contact)
                    .is_some_and(|touch| !touch.panning)
                {
                    let now = self.control_clock();
                    let count = self
                        .interaction
                        .last_tap
                        .as_ref()
                        .filter(|tap| {
                            now - tap.time <= 0.4
                                && distance_squared(tap.position, pointer.position) <= 256.0
                        })
                        .map_or(1, |tap| tap.count.saturating_add(1));
                    self.interaction.last_tap = Some(CompletedTap {
                        position: pointer.position,
                        time: now,
                        count,
                    });
                    self.interaction.emitted_tap = Some((pointer.contact, count));
                }
            }
            InputEvent::PointerCancel { contact } => {
                if self
                    .interaction
                    .pending_touch
                    .as_ref()
                    .is_some_and(|touch| touch.contact == *contact)
                {
                    self.interaction.pending_touch = None;
                }
                self.interaction.emitted_tap = None;
            }
            InputEvent::Focused(false) => {
                self.interaction.pending_touch = None;
                self.interaction.emitted_tap = None;
            }
            _ => {}
        }
        let aqua_core::InputEvent::PointerDown(pointer, button) = event else {
            return;
        };
        if pointer.kind != aqua_core::PointerKind::Mouse {
            return;
        }
        let now = self.control_clock();
        let settings = self.interaction.click_settings;
        let mut distance = settings.max_distance();
        if cfg!(windows) {
            distance.x *= 2.0;
            distance.y *= 2.0;
        }
        let count = self
            .interaction
            .last_click
            .as_ref()
            .filter(|previous| {
                previous.button == *button
                    && now - previous.time < settings.interval().as_secs_f64()
                    && (previous.at.x - pointer.position.x).abs() <= distance.x
                    && (previous.at.y - pointer.position.y).abs() <= distance.y
            })
            .map_or(1, |previous| previous.count.saturating_add(1));
        self.interaction.last_click = Some(PointerClick {
            at: pointer.position,
            button: *button,
            contact: pointer.contact,
            time: now,
            count,
        });
    }

    pub(crate) fn interaction_tap_count(&self, contact: u64) -> Option<usize> {
        self.interaction
            .emitted_tap
            .filter(|(id, _)| *id == contact)
            .map(|(_, count)| count)
    }

    pub(crate) fn interaction_click_count(
        &self,
        pointer: &aqua_core::PointerEvent,
        button: aqua_core::PointerButton,
    ) -> Option<usize> {
        self.interaction
            .last_click
            .as_ref()
            .filter(|click| {
                pointer.kind == aqua_core::PointerKind::Mouse
                    && click.contact == pointer.contact
                    && click.button == button
            })
            .map(|click| click.count)
    }
}

impl super::UiTree {
    pub(crate) fn set_interaction_base_absolute(
        &mut self,
        node: aqua_core::NodeId,
        absolute: Option<crate::style::Absolute>,
    ) {
        if let Some(base) = self
            .interaction
            .nodes
            .get_mut(&node)
            .and_then(|entry| entry.base.as_mut())
        {
            base.layout.absolute = absolute;
        }
    }

    pub fn queue_interaction_binding(&self, node: aqua_core::NodeId, styled: bool) {
        if let (Some(record), Some(paint)) = (self.get(node), self.paint(node)) {
            self.command_sink().interaction(InteractionCommand::Bind {
                node,
                layout: Box::new(record.style.clone()),
                paint: Box::new(paint),
                styled,
            });
        }
    }

    pub fn queue_interaction_event(&self, node: aqua_core::NodeId, event: InteractionEvent) {
        self.command_sink()
            .interaction(InteractionCommand::Event { node, event });
    }

    pub fn queue_interaction_input(&self, event: &aqua_core::InputEvent) {
        use aqua_core::{InputEvent, PointerKind};
        let modality = match event {
            InputEvent::KeyDown(_) => Some(InputModality::Keyboard),
            InputEvent::PointerDown(pointer, _) | InputEvent::PointerMove(pointer) => {
                Some(match pointer.kind {
                    PointerKind::Touch => InputModality::Touch,
                    PointerKind::Mouse | PointerKind::Pen => InputModality::Pointer,
                })
            }
            InputEvent::PointerUp(pointer, _) if pointer.kind == PointerKind::Touch => {
                Some(InputModality::Touch)
            }
            _ => None,
        };
        if let Some(modality) = modality {
            self.command_sink()
                .interaction(InteractionCommand::Modality(modality));
        }
    }

    pub fn queue_interaction_focus(
        &self,
        previous: Option<aqua_core::NodeId>,
        next: Option<aqua_core::NodeId>,
    ) {
        if previous == next {
            return;
        }
        if let Some(node) = previous {
            self.queue_interaction_event(node, InteractionEvent::Focus(false));
        }
        if let Some(node) = next {
            self.queue_interaction_event(node, InteractionEvent::Focus(true));
        }
    }

    pub fn remove_interaction(&mut self, node: aqua_core::NodeId) {
        self.interaction.nodes.remove(&node);
    }

    pub fn interaction_semantics(&self, node: aqua_core::NodeId) -> InteractionSemantics {
        self.interaction
            .nodes
            .get(&node)
            .map(|entry| entry.semantics)
            .unwrap_or_default()
    }

    pub fn apply_interaction_command(&mut self, command: InteractionCommand) {
        let mut store = std::mem::take(&mut self.interaction);
        let mut color_root = None;
        match command {
            InteractionCommand::Modality(modality) => {
                if store.modality != modality {
                    store.modality = modality;
                    color_root = self.root();
                    for (node, entry) in &mut store.nodes {
                        if entry.state.transition(InteractionEvent::Modality(modality)) {
                            if let Some(base) = &entry.base {
                                self.apply_interaction_style(
                                    *node,
                                    &base.styles,
                                    entry.state,
                                    &base.layout,
                                    &base.paint,
                                );
                            }
                        }
                    }
                }
            }
            InteractionCommand::Bind {
                node,
                layout,
                paint,
                styled,
            } => {
                let (layout, paint) = (*layout, *paint);
                if self.get(node).is_some() {
                    let modality = store.modality;
                    let entry = store
                        .nodes
                        .entry(node)
                        .or_insert_with(|| RetainedInteraction {
                            state: InteractionState {
                                focused: self.focused() == Some(node),
                                disabled: self.is_disabled(node),
                                modality,
                                ..InteractionState::default()
                            },
                            base: None,
                            semantics: InteractionSemantics::default(),
                            color_registered: false,
                            own_color: None,
                            own_gpui_color: None,
                            own_source_color: None,
                        });
                    if style_colors(&entry.refinement()) != (None, None, None) {
                        color_root = Some(node);
                    }
                    let had_base = entry.base.is_some();
                    entry.base = (styled || had_base).then(|| {
                        Box::new(InteractionBase {
                            styles: InteractionStyles::default(),
                            layout,
                            paint,
                        })
                    });
                    entry.state.focused = self.focused() == Some(node);
                    entry
                        .state
                        .transition(InteractionEvent::Disabled(self.is_disabled(node)));
                    if entry.semantics != InteractionSemantics::default() {
                        entry.semantics = InteractionSemantics::default();
                        self.touch_paint(node);
                    }
                    if let Some(base) = &entry.base {
                        self.apply_interaction_style(
                            node,
                            &base.styles,
                            entry.state,
                            &base.layout,
                            &base.paint,
                        );
                    }
                    if !styled {
                        entry.base = None;
                    }
                }
            }
            InteractionCommand::Style { node, kind, style } => {
                let current = self
                    .get(node)
                    .and_then(|record| self.paint(node).map(|paint| (record.style.clone(), paint)));
                if let (Some(entry), Some((layout, paint))) = (store.nodes.get_mut(&node), current)
                {
                    let affects_color = style_colors(&style) != (None, None, None);
                    let base = entry.base.get_or_insert_with(|| {
                        Box::new(InteractionBase {
                            styles: InteractionStyles::default(),
                            layout,
                            paint,
                        })
                    });
                    base.styles.set(kind, *style);
                    if affects_color {
                        color_root = Some(node);
                    }
                    self.apply_interaction_style(
                        node,
                        &base.styles,
                        entry.state,
                        &base.layout,
                        &base.paint,
                    );
                }
            }
            InteractionCommand::Event { node, event } => {
                if let Some(entry) = store
                    .nodes
                    .get_mut(&node)
                    .filter(|_| self.get(node).is_some())
                {
                    if matches!(event, InteractionEvent::Focus(_)) {
                        self.touch_paint(node);
                    }
                    let before_color = style_colors(&entry.refinement());
                    if entry.state.transition(event) {
                        if before_color != style_colors(&entry.refinement()) {
                            color_root = Some(node);
                        }
                        if let Some(base) = &entry.base {
                            self.apply_interaction_style(
                                node,
                                &base.styles,
                                entry.state,
                                &base.layout,
                                &base.paint,
                            );
                        }
                    }
                }
            }
            InteractionCommand::TypographyColor {
                node,
                own,
                own_gpui,
                own_source,
            } => {
                if let Some(entry) = store.nodes.get_mut(&node) {
                    entry.color_registered = true;
                    entry.own_color = own;
                    entry.own_gpui_color = own_gpui;
                    entry.own_source_color = own_source;
                }
                if self.has_interaction_color(&store, node) {
                    color_root = Some(node);
                }
            }
            InteractionCommand::Semantics { node, value } => {
                if let Some(record) = self.get(node) {
                    let mut semantics = record.semantics.clone();
                    match value {
                        InteractionSemantic::Selected(selected) => {
                            if semantics.selected != selected {
                                semantics.selected = selected;
                                self.set_semantics(node, semantics);
                            }
                        }
                        InteractionSemantic::Value(value) => {
                            if semantics.value_text != value {
                                semantics.value_text = value;
                                self.set_semantics(node, semantics);
                            }
                        }
                        InteractionSemantic::Expanded(value) => {
                            if let Some(entry) = store.nodes.get_mut(&node) {
                                if entry.semantics.expanded != value {
                                    entry.semantics.expanded = value;
                                    self.touch_paint(node);
                                }
                            }
                        }
                        InteractionSemantic::Level(value) => {
                            if let Some(entry) = store.nodes.get_mut(&node) {
                                if entry.semantics.level != value {
                                    entry.semantics.level = value;
                                    self.touch_paint(node);
                                }
                            }
                        }
                        InteractionSemantic::Checked(value) => {
                            if let Some(entry) = store.nodes.get_mut(&node) {
                                if entry.semantics.checked != value {
                                    entry.semantics.checked = value;
                                    self.touch_paint(node);
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(root) = color_root {
            self.refresh_interaction_colors(&store, root);
        }
        self.interaction = store;
    }

    fn has_interaction_color(&self, store: &InteractionStore, node: aqua_core::NodeId) -> bool {
        let mut current = Some(node);
        while let Some(id) = current {
            if store
                .nodes
                .get(&id)
                .is_some_and(|entry| style_colors(&entry.refinement()) != (None, None, None))
            {
                return true;
            }
            current = self.get(id).and_then(|record| record.parent);
        }
        false
    }

    fn refresh_interaction_colors(&mut self, store: &InteractionStore, root: aqua_core::NodeId) {
        let mut ancestors = Vec::new();
        let mut current = self.get(root).and_then(|record| record.parent);
        while let Some(node) = current {
            ancestors.push(node);
            current = self.get(node).and_then(|record| record.parent);
        }
        let resolve = |node: aqua_core::NodeId,
                       inherited: (
            aqua_core::Oklch,
            Option<aqua_core::GpuiHsla>,
            Option<aqua_core::GpuiTextColor>,
        )| {
            store.nodes.get(&node).map_or(inherited, |entry| {
                let (color, gpui_color, source_color) = style_colors(&entry.refinement());
                (
                    color.or(entry.own_color).unwrap_or(inherited.0),
                    gpui_color.or(entry.own_gpui_color).unwrap_or(inherited.1),
                    source_color
                        .or(entry.own_source_color)
                        .unwrap_or(inherited.2),
                )
            })
        };
        let mut inherited = (aqua_core::Oklch::WHITE, None, None);
        for node in ancestors.into_iter().rev() {
            inherited = resolve(node, inherited);
        }
        let mut pending = vec![(root, inherited)];
        while let Some((node, parent_color)) = pending.pop() {
            let color = resolve(node, parent_color);
            let Some(record) = self.get(node) else {
                continue;
            };
            let children = record.children.clone();
            let gpui_color = color.1.map(|value| [value.h, value.s, value.l, value.a]);
            let source_color = color.2.map(|value| {
                [
                    value.hue_degrees,
                    value.saturation,
                    value.lightness,
                    value.alpha,
                ]
            });
            let change = store
                .nodes
                .get(&node)
                .is_some_and(|entry| entry.color_registered)
                && matches!(&record.kind, super::NodeKind::Text(text) if text.color != color.0 || text.text_style.gpui_color != gpui_color || text.text_style.gpui_source_color != source_color);
            pending.extend(children.into_iter().map(|child| (child, color)));
            if !change {
                continue;
            }
            if let Some(super::NodeKind::Text(text)) =
                self.get_mut(node).map(|record| &mut record.kind)
            {
                text.color = color.0;
                text.text_style.gpui_color = gpui_color;
                text.text_style.gpui_source_color = source_color;
            }
        }
    }
}

fn style_colors(
    style: &Style,
) -> (
    Option<aqua_core::Oklch>,
    Option<Option<aqua_core::GpuiHsla>>,
    Option<Option<aqua_core::GpuiTextColor>>,
) {
    let gpui = style.gpui_text_color_opt().or_else(|| {
        style.text_style_opt().and_then(|text| {
            text.gpui_color
                .map(|[h, s, l, a]| Some(aqua_core::GpuiHsla { h, s, l, a }))
        })
    });
    let source = style.gpui_source_text_color_opt().or_else(|| {
        style.text_style_opt().and_then(|text| {
            text.gpui_source_color
                .map(|[hue_degrees, saturation, lightness, alpha]| {
                    Some(aqua_core::GpuiTextColor {
                        hue_degrees,
                        saturation,
                        lightness,
                        alpha,
                    })
                })
        })
    });
    (style.text_color_opt(), gpui, source)
}
