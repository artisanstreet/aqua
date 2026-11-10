use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::marker::PhantomData;
use std::rc::Rc;

use aqua_core::{NodeId, Oklch, Point};
use aqua_reactive::{BindingCause, BindingSink, ReadSignal, Scope};

use crate::style::{LayoutStyle, Paint};
use crate::tree::{CommandSink, UiCommand, UiTree};

pub trait MountBinding {
    fn validate(&self, _: &crate::tree::NodeKind) -> Result<(), &'static str> {
        Ok(())
    }
    fn owns_children(&self) -> bool {
        false
    }
    fn dependencies(&self) -> Vec<aqua_reactive::NodeId>;
    fn mount(
        &self,
        tree: &mut UiTree,
        node: NodeId,
        scope: &Scope,
        typography: &crate::element::TypoChain,
    );
}

#[derive(Clone, Copy)]
pub enum PropertyTarget {
    Any,
    Text,
    Scroll,
}

pub trait Property<T>: Copy + 'static {
    fn target(self) -> PropertyTarget {
        PropertyTarget::Any
    }
    fn encode(self, value: &T) -> PropertyValue;
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropertyValue {
    Text(String),
    Paint(Box<Paint>),
    Layout(Box<LayoutStyle>),
    Opacity(f32),
    Background(Oklch),
    TextColor(Oklch),
    Label(String),
    Scroll(Point),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyUpdate {
    pub node: NodeId,
    pub cause: BindingCause,
    pub value: PropertyValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateTrace {
    pub node: NodeId,
    pub cause: BindingCause,
    pub property: &'static str,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkStage {
    PaintInvalidated,
    LayoutInvalidated,
    LayoutCommitted,
    TextInspected,
    TextShaped,
    ScenePatched,
    SceneRebuilt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkTrace {
    pub node: NodeId,
    pub sources: Vec<BindingCause>,
    pub stage: WorkStage,
}

#[derive(Default, Debug)]
struct TraceState {
    entries: VecDeque<UpdateTrace>,
    work: VecDeque<WorkTrace>,
    causes: HashMap<NodeId, Vec<BindingCause>>,
    current: Option<BindingCause>,
    enabled: bool,
}

#[derive(Clone, Default, Debug)]
pub struct UpdateLog {
    state: Rc<RefCell<TraceState>>,
}

impl UpdateLog {
    pub fn set_enabled(&self, enabled: bool) {
        let mut state = self.state.borrow_mut();
        state.enabled = enabled;
        if !enabled {
            state.entries.clear();
            state.work.clear();
            state.causes.clear();
        }
    }
    pub fn drain(&self) -> Vec<UpdateTrace> {
        self.state.borrow_mut().entries.drain(..).collect()
    }
    pub fn drain_work(&self) -> Vec<WorkTrace> {
        self.state.borrow_mut().work.drain(..).collect()
    }
    pub(crate) fn finish(&self) {
        self.state.borrow_mut().causes.clear();
    }
    pub(crate) fn forget(&self, node: NodeId) {
        self.state.borrow_mut().causes.remove(&node);
    }
    pub(crate) fn causes(&self, node: NodeId) -> Vec<BindingCause> {
        self.state
            .borrow()
            .causes
            .get(&node)
            .cloned()
            .unwrap_or_default()
    }
    pub(crate) fn enabled(&self) -> bool {
        self.state.borrow().enabled
    }

    pub(crate) fn enter(&self, cause: BindingCause) -> SourceGuard {
        let previous = self.state.borrow_mut().current.replace(cause);
        SourceGuard {
            log: self.clone(),
            previous,
        }
    }

    pub(crate) fn work(&self, node: NodeId, stage: WorkStage, inherited: Vec<BindingCause>) {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            return;
        }
        let mut sources = inherited;
        if let Some(cause) = state.current {
            sources.push(cause);
        }
        let known = state.causes.entry(node).or_default();
        for cause in sources {
            if !known.contains(&cause) {
                known.push(cause);
            }
        }
        let sources = known.clone();
        if state.work.len() == 4096 {
            state.work.pop_front();
        }
        state.work.push_back(WorkTrace {
            node,
            sources,
            stage,
        });
    }

    pub(crate) fn record(&self, entry: UpdateTrace) {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            return;
        }
        if state.entries.len() == 1024 {
            state.entries.pop_front();
        }
        state.entries.push_back(entry);
    }
}

pub(crate) struct SourceGuard {
    log: UpdateLog,
    previous: Option<BindingCause>,
}

impl Drop for SourceGuard {
    fn drop(&mut self) {
        self.log.state.borrow_mut().current = self.previous;
    }
}

macro_rules! property {
    ($name:ident, $value:ty, $variant:ident, $target:ident) => {
        #[derive(Clone, Copy, Debug)]
        pub struct $name;
        impl Property<$value> for $name {
            fn target(self) -> PropertyTarget {
                PropertyTarget::$target
            }
            fn encode(self, value: &$value) -> PropertyValue {
                PropertyValue::$variant(value.clone())
            }
        }
    };
}

property!(Text, String, Text, Text);
#[derive(Clone, Copy, Debug)]
pub struct Appearance;
impl Property<Paint> for Appearance {
    fn encode(self, value: &Paint) -> PropertyValue {
        PropertyValue::Paint(Box::new(value.clone()))
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Layout;
impl Property<LayoutStyle> for Layout {
    fn encode(self, value: &LayoutStyle) -> PropertyValue {
        PropertyValue::Layout(Box::new(value.clone()))
    }
}
property!(Opacity, f32, Opacity, Any);
property!(Background, Oklch, Background, Any);
property!(TextColor, Oklch, TextColor, Text);
property!(Label, String, Label, Any);
property!(Scroll, Point, Scroll, Scroll);

pub fn bind_property<T: Clone + 'static, P: Property<T>>(
    scope: &Scope,
    commands: CommandSink,
    node: NodeId,
    source: &ReadSignal<T>,
    property: P,
) -> aqua_reactive::EffectHandle {
    let version = source.version();
    let value = source.get();
    let mut sink = PropertySink::new(commands, node, property);
    sink.write(
        &value,
        BindingCause {
            source: source.node_id(),
            version,
        },
    );
    scope.bind_at_version(source, sink, version)
}

pub struct Binding<T, P> {
    source: ReadSignal<T>,
    property: P,
}

impl<T, P> Binding<T, P> {
    pub fn new(source: ReadSignal<T>, property: P) -> Self {
        Self { source, property }
    }
}

impl<T: Clone + 'static, P: Property<T>> MountBinding for Binding<T, P> {
    fn validate(&self, kind: &crate::tree::NodeKind) -> Result<(), &'static str> {
        match (self.property.target(), kind) {
            (PropertyTarget::Any, _)
            | (PropertyTarget::Text, crate::tree::NodeKind::Text(_))
            | (PropertyTarget::Scroll, crate::tree::NodeKind::Scroll(_)) => Ok(()),
            _ => Err("bound property is unsupported by the target node"),
        }
    }

    fn dependencies(&self) -> Vec<aqua_reactive::NodeId> {
        vec![self.source.node_id()]
    }

    fn mount(&self, tree: &mut UiTree, node: NodeId, scope: &Scope, _: &crate::element::TypoChain) {
        bind_property(
            scope,
            tree.command_sink(),
            node,
            &self.source,
            self.property,
        );
    }
}

pub struct PropertySink<T, P> {
    commands: CommandSink,
    node: NodeId,
    property: P,
    value: PhantomData<fn(T)>,
}

impl<T, P> PropertySink<T, P> {
    pub fn new(commands: CommandSink, node: NodeId, property: P) -> Self {
        Self {
            commands,
            node,
            property,
            value: PhantomData,
        }
    }
}

impl<T: 'static, P: Property<T>> BindingSink<T> for PropertySink<T, P> {
    fn write(&mut self, value: &T, cause: BindingCause) {
        self.commands.push(UiCommand::Property(PropertyUpdate {
            node: self.node,
            cause,
            value: self.property.encode(value),
        }));
    }
}

pub(crate) fn apply(tree: &mut UiTree, update: PropertyUpdate) {
    let _cause = tree.update_log().enter(update.cause);
    let before = tree.invalidation_epoch();
    let node = update.node;
    let property = match update.value {
        PropertyValue::Text(value) => {
            tree.set_text(node, value);
            "text"
        }
        PropertyValue::Paint(value) => {
            tree.apply_bound_style(node, |_, paint| *paint = *value);
            "paint"
        }
        PropertyValue::Layout(value) => {
            tree.apply_bound_style(node, |layout, _| *layout = *value);
            "layout"
        }
        PropertyValue::Opacity(value) => {
            if value.is_finite() {
                tree.apply_bound_style(node, |_, paint| {
                    paint.opacity = Some(value.clamp(0.0, 1.0))
                });
            }
            "opacity"
        }
        PropertyValue::Background(value) => {
            tree.apply_bound_style(node, |_, paint| paint.fill = Some(value));
            "background"
        }
        PropertyValue::TextColor(value) => {
            tree.set_bound_text_color(node, value);
            "text-color"
        }
        PropertyValue::Label(value) => {
            if let Some(record) = tree.get(node) {
                let mut semantics = record.semantics.clone();
                semantics.label = value;
                tree.set_semantics(node, semantics);
            }
            "label"
        }
        PropertyValue::Scroll(value) => {
            tree.scroll_to(node, value);
            "scroll"
        }
    };
    tree.update_log().record(UpdateTrace {
        node,
        cause: update.cause,
        property,
        changed: before != tree.invalidation_epoch(),
    });
}

pub(crate) struct LayoutBoundary(pub aqua_core::Size);

impl MountBinding for LayoutBoundary {
    fn validate(&self, _: &crate::tree::NodeKind) -> Result<(), &'static str> {
        match self.0.w.is_finite() && self.0.h.is_finite() && self.0.w >= 0.0 && self.0.h >= 0.0 {
            true => Ok(()),
            false => Err("layout boundary requires a finite non-negative size"),
        }
    }

    fn dependencies(&self) -> Vec<aqua_reactive::NodeId> {
        Vec::new()
    }
    fn mount(&self, tree: &mut UiTree, node: NodeId, _: &Scope, _: &crate::element::TypoChain) {
        assert!(
            tree.set_layout_boundary(node, self.0),
            "layout boundary requires a finite non-negative size"
        );
    }
}
