use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use aqua_core::{NodeId, Size};
use aqua_reactive::{BindingCause, BindingSink, ReadSignal, Scope};

use crate::binding::View;
use crate::element::{lower_specs_in, Element, TypoChain};
use crate::layout::TextMeasurer;
use crate::program::MountBinding;
use crate::tree::{CommandSink, UiCommand, UiTree};

pub trait MountedRegion {
    fn needs_layout(&self) -> bool;
    fn update(&mut self, tree: &mut UiTree) -> Result<(), String>;
    fn layout(
        &mut self,
        tree: &mut UiTree,
        viewport: Size,
        measurer: &dyn TextMeasurer,
    ) -> Result<(), String>;
}

struct RegionSink {
    sink: CommandSink,
    node: NodeId,
    pending: Rc<Cell<bool>>,
}

impl<T: 'static> BindingSink<T> for RegionSink {
    fn write(&mut self, _: &T, cause: BindingCause) {
        if !self.pending.replace(true) {
            self.sink.push(UiCommand::RefreshRegion {
                node: self.node,
                cause,
            });
        }
    }
}

type Branch = Rc<dyn Fn(&Scope) -> Element>;

struct Conditional {
    source: ReadSignal<bool>,
    yes: Branch,
    no: Branch,
}

struct MountedConditional {
    source: ReadSignal<bool>,
    yes: Branch,
    no: Branch,
    owner: Scope,
    typography: TypoChain,
    parent: NodeId,
    current: Option<(bool, NodeId, View)>,
    pending: Rc<Cell<bool>>,
}

pub fn when(
    source: ReadSignal<bool>,
    yes: impl Fn(&Scope) -> Element + 'static,
    no: impl Fn(&Scope) -> Element + 'static,
) -> Element {
    crate::element::ui::Box::new()
        .style(crate::style::Style::new().opacity(1.0))
        .build()
        .install(Conditional {
            source,
            yes: Rc::new(yes),
            no: Rc::new(no),
        })
}

impl MountBinding for Conditional {
    fn owns_children(&self) -> bool {
        true
    }
    fn dependencies(&self) -> Vec<aqua_reactive::NodeId> {
        vec![self.source.node_id()]
    }
    fn mount(&self, tree: &mut UiTree, node: NodeId, scope: &Scope, typography: &TypoChain) {
        let pending = Rc::new(Cell::new(false));
        let version = self.source.version();
        tree.install_region(
            node,
            Box::new(MountedConditional {
                source: self.source.clone(),
                yes: self.yes.clone(),
                no: self.no.clone(),
                owner: scope.clone(),
                typography: typography.clone(),
                parent: node,
                current: None,
                pending: pending.clone(),
            }),
        );
        scope.bind_at_version(
            &self.source,
            RegionSink {
                sink: tree.command_sink(),
                node,
                pending,
            },
            version,
        );
    }
}

impl MountedRegion for MountedConditional {
    fn needs_layout(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|(_, _, view)| view.has_collections())
    }
    fn update(&mut self, tree: &mut UiTree) -> Result<(), String> {
        self.pending.set(false);
        let Some(value) = self.source.try_get() else {
            return Ok(());
        };
        if self
            .current
            .as_ref()
            .is_some_and(|(current, _, _)| *current == value)
        {
            return Ok(());
        }
        let owner = self.owner.child();
        let element = match value {
            true => (self.yes)(&owner),
            false => (self.no)(&owner),
        };
        let mut specs = match lower_specs_in(&element, &self.typography) {
            Ok(specs) => specs,
            Err(error) => {
                owner.dispose();
                return Err(error.to_string());
            }
        };
        if specs.len() != 1 {
            owner.dispose();
            return Err("a conditional branch must have one root".into());
        }
        let Some(node) = tree.insert_spec(self.parent, 0, specs.remove(0)) else {
            owner.dispose();
            return Err("conditional owner was removed".into());
        };
        let mut view = View::new(owner, tree.command_sink());
        if let Err(error) = view.bind_existing_in(tree, node, element, &self.typography) {
            view.dispose();
            tree.remove(node);
            return Err(error.to_string());
        }
        if let Some((_, previous, previous_view)) = self.current.take() {
            previous_view.dispose();
            tree.remove(previous);
        }
        self.current = Some((value, node, view));
        Ok(())
    }

    fn layout(
        &mut self,
        tree: &mut UiTree,
        viewport: Size,
        measurer: &dyn TextMeasurer,
    ) -> Result<(), String> {
        match &mut self.current {
            Some((_, _, view)) => view.compute_layout(tree, viewport, measurer),
            None => Ok(()),
        }
    }
}

impl Drop for MountedConditional {
    fn drop(&mut self) {
        if let Some((_, _, view)) = self.current.take() {
            view.dispose();
        }
    }
}

pub(crate) type Regions = HashMap<NodeId, Box<dyn MountedRegion>>;

pub(crate) fn invalidate<T: Clone + 'static>(
    tree: &mut UiTree,
    node: NodeId,
    scope: &Scope,
    source: &ReadSignal<T>,
    pending: Rc<Cell<bool>>,
    version: u64,
) {
    scope.bind_at_version(
        source,
        RegionSink {
            sink: tree.command_sink(),
            node,
            pending,
        },
        version,
    );
}
