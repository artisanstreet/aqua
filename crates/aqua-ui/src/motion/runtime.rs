use std::collections::HashMap;
use std::rc::Rc;

use aqua_core::{NodeId, Oklch};
use aqua_layout::edge::EdgeAnimator;
use aqua_motion::{
    AppMotion, Binding, MotionEvent, MotionKind, MotionValue, Property, Seconds, TrackId,
};
use aqua_style::motion::{AnimatedProperty, MotionRef, MotionSpec, MotionTheme};

use crate::tree::{MotionGroups, NodeKind, UiTree};

struct PropState {
    track: TrackId,
    target: MotionValue,
    motion: MotionRef,
}

#[derive(Default)]
struct NodeState {
    props: HashMap<AnimatedProperty, PropState>,
    frame: Option<MotionRef>,
    exit: Option<Vec<TrackId>>,
}

pub struct MotionRuntime {
    theme: MotionTheme,
    edges: EdgeAnimator,
    nodes: HashMap<NodeId, NodeState>,
    seeded: bool,
}

impl Default for MotionRuntime {
    fn default() -> Self {
        Self::new(MotionTheme::default())
    }
}

const fn binding_property(property: AnimatedProperty) -> Option<Property> {
    match property {
        AnimatedProperty::Opacity => Some(Property::Opacity),
        AnimatedProperty::Fill => Some(Property::Fill),
        AnimatedProperty::BorderColor => Some(Property::BorderColor),
        AnimatedProperty::TextColor => Some(Property::TextColor),
        AnimatedProperty::Translate => Some(Property::Translate),
        AnimatedProperty::Frame => None,
    }
}

fn faded(color: Oklch) -> Oklch {
    Oklch {
        alpha: 0.0,
        ..color
    }
}

fn resolved_value(tree: &UiTree, node: NodeId, property: AnimatedProperty) -> Option<MotionValue> {
    let paint = tree.paint(node)?;
    match property {
        AnimatedProperty::Opacity => Some(MotionValue::Scalar(paint.opacity.unwrap_or(1.0))),
        AnimatedProperty::Fill => paint.fill.map(MotionValue::Color),
        AnimatedProperty::BorderColor => {
            paint.border.map(|border| MotionValue::Color(border.color))
        }
        AnimatedProperty::TextColor => match tree.get(node).map(|record| &record.kind) {
            Some(NodeKind::Text(text)) => Some(MotionValue::Color(text.color)),
            _ => None,
        },
        AnimatedProperty::Translate => {
            Some(paint.transform.map_or(MotionValue::Pair(0.0, 0.0), |t| {
                MotionValue::Pair(t.tx, t.ty)
            }))
        }
        AnimatedProperty::Frame => None,
    }
}

impl MotionRuntime {
    #[must_use]
    pub fn new(theme: MotionTheme) -> Self {
        Self {
            theme,
            edges: EdgeAnimator::new(),
            nodes: HashMap::new(),
            seeded: false,
        }
    }

    #[must_use]
    pub fn theme(&self) -> &MotionTheme {
        &self.theme
    }

    pub fn seed(&mut self) {
        self.seeded = true;
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.nodes.is_empty()
    }

    #[must_use]
    pub fn owns(&self, track: TrackId) -> bool {
        self.nodes.values().any(|state| {
            state.props.values().any(|prop| prop.track == track)
                || state
                    .exit
                    .as_ref()
                    .is_some_and(|tracks| tracks.contains(&track))
        }) || self
            .nodes
            .keys()
            .any(|node| self.edges.tracks(*node).contains(&track))
    }

    #[must_use]
    pub fn bindings(&self) -> Vec<(TrackId, Binding)> {
        let mut out = Vec::new();
        for (node, state) in &self.nodes {
            for (property, prop) in &state.props {
                if let Some(property) = binding_property(*property) {
                    out.push((
                        prop.track,
                        Binding {
                            node: *node,
                            property,
                        },
                    ));
                }
            }
            for edge in aqua_layout::edge::EDGES {
                if let Some(track) = self.edges.track(*node, edge) {
                    out.push((
                        track,
                        Binding {
                            node: *node,
                            property: Property::Edge(edge),
                        },
                    ));
                }
            }
        }
        out
    }

    #[must_use]
    pub fn groups(&self, tree: &UiTree) -> HashMap<NodeId, MotionGroups> {
        let mut out = HashMap::new();
        for node in tree.traverse() {
            let Some(spec) = tree.motion_spec(node) else {
                continue;
            };
            let opacity = (spec.presence.fades()
                && (spec.presence.enter_motion().is_some()
                    || spec.presence.exit_motion().is_some()))
                || spec
                    .animate
                    .properties()
                    .iter()
                    .any(|(property, _)| *property == AnimatedProperty::Opacity);
            let translate = spec.presence.slide_offset().is_some()
                || spec
                    .animate
                    .properties()
                    .iter()
                    .any(|(property, _)| *property == AnimatedProperty::Translate);
            if opacity || translate {
                out.insert(node, MotionGroups { opacity, translate });
            }
        }
        out
    }

    #[must_use]
    pub fn target_of(&self, node: NodeId, property: AnimatedProperty) -> Option<MotionValue> {
        self.nodes
            .get(&node)
            .and_then(|state| state.props.get(&property))
            .map(|prop| prop.target)
    }

    #[must_use]
    pub fn track_of(&self, node: NodeId, property: AnimatedProperty) -> Option<TrackId> {
        self.nodes
            .get(&node)
            .and_then(|state| state.props.get(&property))
            .map(|prop| prop.track)
    }

    #[must_use]
    pub fn edge_animator(&self) -> &EdgeAnimator {
        &self.edges
    }

    pub fn set_theme(&mut self, tracks: &mut AppMotion, theme: MotionTheme, at: Seconds) {
        let previous = std::mem::replace(&mut self.theme, theme);
        let changed = |motion: MotionRef, theme: &MotionTheme| {
            matches!(motion, MotionRef::Token(_))
                && previous.resolve(motion) != theme.resolve(motion)
        };
        for (node, state) in &mut self.nodes {
            for prop in state.props.values_mut() {
                if changed(prop.motion, &self.theme) && !tracks.is_settled(prop.track) {
                    let kind = self.theme.resolve(prop.motion).kind();
                    let _ = tracks.set_target_with(prop.track, prop.target, kind, at);
                }
            }
            if let Some(frame) = state.frame {
                if changed(frame, &self.theme) {
                    let kind = self.theme.resolve(frame).kind();
                    self.edges.retime(tracks, *node, kind, at);
                }
            }
        }
    }

    pub fn reconcile(&mut self, tracks: &mut AppMotion, tree: &UiTree, at: Seconds) {
        let mut live: Vec<(NodeId, Rc<MotionSpec>)> = Vec::new();
        for node in tree.traverse() {
            if let Some(spec) = tree.motion_spec(node) {
                live.push((node, spec));
            }
        }
        for (node, spec) in &live {
            self.sync_node(tracks, tree, *node, spec, at);
        }
        let gone: Vec<NodeId> = self
            .nodes
            .keys()
            .filter(|node| !live.iter().any(|(live_node, _)| live_node == *node))
            .copied()
            .collect();
        for node in gone {
            self.nodes.remove(&node);
            self.edges.forget(tracks, node);
        }
        self.seeded = true;
    }

    pub fn handle(
        &mut self,
        tracks: &mut AppMotion,
        tree: &mut UiTree,
        event: MotionEvent,
    ) -> Vec<NodeId> {
        tracks.handle(event);
        let finished: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, state)| {
                state
                    .exit
                    .as_ref()
                    .is_some_and(|exit| exit.iter().all(|track| tracks.is_settled(*track)))
            })
            .map(|(node, _)| *node)
            .collect();
        for node in &finished {
            self.nodes.remove(node);
            self.edges.forget(tracks, *node);
            tree.finish_exit(*node);
        }
        finished
    }

    fn resting_translate(tree: &UiTree, node: NodeId) -> (f32, f32) {
        match resolved_value(tree, node, AnimatedProperty::Translate) {
            Some(MotionValue::Pair(x, y)) => (x, y),
            _ => (0.0, 0.0),
        }
    }

    fn rest_translate(state: &NodeState, tree: &UiTree, node: NodeId) -> (f32, f32) {
        match state
            .props
            .get(&AnimatedProperty::Translate)
            .map(|prop| prop.target)
        {
            Some(MotionValue::Pair(x, y)) => (x, y),
            _ => Self::resting_translate(tree, node),
        }
    }

    fn kind_of(&self, reference: MotionRef) -> MotionKind {
        self.theme.resolve(reference).kind()
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_prop(
        &mut self,
        tracks: &mut AppMotion,
        state: &mut NodeState,
        node: NodeId,
        property: AnimatedProperty,
        initial: MotionValue,
        motion: MotionRef,
        at: Seconds,
    ) -> TrackId {
        let kind = self.kind_of(motion);
        match state.props.get(&property) {
            Some(prop) => prop.track,
            None => {
                let track = tracks.spawn(
                    Some(node),
                    binding_property(property).map(|property| Binding { node, property }),
                    initial,
                    kind,
                    at,
                );
                state.props.insert(
                    property,
                    PropState {
                        track,
                        target: initial,
                        motion,
                    },
                );
                track
            }
        }
    }

    fn retarget(
        &mut self,
        tracks: &mut AppMotion,
        state: &mut NodeState,
        property: AnimatedProperty,
        to: MotionValue,
        motion: MotionRef,
        at: Seconds,
    ) -> Option<TrackId> {
        let kind = self.kind_of(motion);
        let prop = state.props.get_mut(&property)?;
        prop.motion = motion;
        if prop.target != to {
            prop.target = to;
            let _ = tracks.set_target_with(prop.track, to, kind, at);
        }
        Some(prop.track)
    }

    fn sync_node(
        &mut self,
        tracks: &mut AppMotion,
        tree: &UiTree,
        node: NodeId,
        spec: &MotionSpec,
        at: Seconds,
    ) {
        let entered = !self.nodes.contains_key(&node);
        let mut state = self.nodes.remove(&node).unwrap_or_default();
        let opacity = resolved_value(tree, node, AnimatedProperty::Opacity)
            .unwrap_or(MotionValue::Scalar(1.0));
        if tree.is_exiting(node) {
            if state.exit.is_none() {
                let exit = spec
                    .presence
                    .exit_motion()
                    .unwrap_or(MotionRef::Literal(aqua_style::motion::Motion::INSTANT));
                let mut exit_tracks = Vec::new();
                let kind = self.kind_of(exit);
                if spec.presence.fades() {
                    let at_rest = state
                        .props
                        .get(&AnimatedProperty::Opacity)
                        .map_or(opacity, |prop| prop.target);
                    let track = self.ensure_prop(
                        tracks,
                        &mut state,
                        node,
                        AnimatedProperty::Opacity,
                        at_rest,
                        exit,
                        at,
                    );
                    if let Some(prop) = state.props.get_mut(&AnimatedProperty::Opacity) {
                        prop.target = MotionValue::Scalar(0.0);
                        prop.motion = exit;
                    }
                    let _ = tracks.set_target_with(track, MotionValue::Scalar(0.0), kind, at);
                    exit_tracks.push(track);
                }
                if let Some((dx, dy)) = spec.presence.slide_offset() {
                    let rest = Self::rest_translate(&state, tree, node);
                    let away = MotionValue::Pair(rest.0 + dx, rest.1 + dy);
                    let slide = self.ensure_prop(
                        tracks,
                        &mut state,
                        node,
                        AnimatedProperty::Translate,
                        MotionValue::Pair(rest.0, rest.1),
                        exit,
                        at,
                    );
                    if let Some(prop) = state.props.get_mut(&AnimatedProperty::Translate) {
                        prop.target = away;
                        prop.motion = exit;
                    }
                    let _ = tracks.set_target_with(slide, away, kind, at);
                    exit_tracks.push(slide);
                }
                state.exit = Some(exit_tracks);
            }
            self.nodes.insert(node, state);
            return;
        }
        if state.exit.take().is_some() {
            let motion = spec
                .presence
                .enter_motion()
                .or(spec.animate.motion_of(AnimatedProperty::Opacity))
                .or(spec.presence.exit_motion())
                .unwrap_or(MotionRef::Literal(aqua_style::motion::Motion::INSTANT));
            if spec.presence.fades() {
                self.retarget(
                    tracks,
                    &mut state,
                    AnimatedProperty::Opacity,
                    opacity,
                    motion,
                    at,
                );
            }
            if spec.presence.slide_offset().is_some() {
                let rest = Self::resting_translate(tree, node);
                self.retarget(
                    tracks,
                    &mut state,
                    AnimatedProperty::Translate,
                    MotionValue::Pair(rest.0, rest.1),
                    motion,
                    at,
                );
            }
        }
        if entered && self.seeded {
            if let Some(enter) = spec.presence.enter_motion() {
                if spec.presence.fades() {
                    self.ensure_prop(
                        tracks,
                        &mut state,
                        node,
                        AnimatedProperty::Opacity,
                        MotionValue::Scalar(0.0),
                        enter,
                        at,
                    );
                    self.retarget(
                        tracks,
                        &mut state,
                        AnimatedProperty::Opacity,
                        opacity,
                        enter,
                        at,
                    );
                }
                if let Some((dx, dy)) = spec.presence.slide_offset() {
                    let rest = Self::resting_translate(tree, node);
                    self.ensure_prop(
                        tracks,
                        &mut state,
                        node,
                        AnimatedProperty::Translate,
                        MotionValue::Pair(rest.0 + dx, rest.1 + dy),
                        enter,
                        at,
                    );
                    self.retarget(
                        tracks,
                        &mut state,
                        AnimatedProperty::Translate,
                        MotionValue::Pair(rest.0, rest.1),
                        enter,
                        at,
                    );
                }
            }
        }
        for (property, motion) in spec.animate.properties() {
            match property {
                AnimatedProperty::Frame => {
                    state.frame = Some(motion);
                    if let Some(rect) = tree.rect(node) {
                        let kind = self.kind_of(motion);
                        self.edges.observe(tracks, node, rect, kind, at);
                    }
                }
                _ => {
                    let slot = (property, resolved_value(tree, node, property));
                    self.sync_property(tracks, &mut state, node, slot, motion, entered, at)
                }
            }
        }
        self.nodes.insert(node, state);
    }

    #[allow(clippy::too_many_arguments)]
    fn sync_property(
        &mut self,
        tracks: &mut AppMotion,
        state: &mut NodeState,
        node: NodeId,
        (property, current): (AnimatedProperty, Option<MotionValue>),
        motion: MotionRef,
        entered: bool,
        at: Seconds,
    ) {
        match (state.props.contains_key(&property), current) {
            (false, Some(value)) => {
                let initial = match (entered, value) {
                    (false, MotionValue::Color(color)) => MotionValue::Color(faded(color)),
                    _ => value,
                };
                self.ensure_prop(tracks, state, node, property, initial, motion, at);
                self.retarget(tracks, state, property, value, motion, at);
            }
            (true, Some(value)) => {
                self.retarget(tracks, state, property, value, motion, at);
            }
            (true, None) => {
                let faded_target = match state.props.get(&property).map(|prop| prop.target) {
                    Some(MotionValue::Color(color)) => Some(MotionValue::Color(faded(color))),
                    _ => None,
                };
                if let Some(target) = faded_target {
                    self.retarget(tracks, state, property, target, motion, at);
                }
            }
            (false, None) => {}
        }
    }
}
