use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

use aqua_core::{NodeId, Oklch, Srgba8};
use aqua_reactive::{DepTuple, EffectHandle, Scope};

use crate::{CommandSink, UiCommand, UiTree};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FillError {
    ForeignTree,
    Count { expected: usize, actual: usize },
    StaleNode(NodeId),
    DuplicateNode(NodeId),
    InvalidColor { index: usize },
}

impl fmt::Display for FillError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for FillError {}

struct Group {
    owner: Rc<()>,
    nodes: Vec<NodeId>,
    epoch: Cell<u64>,
    recycled: RefCell<Vec<Oklch>>,
    recycled_srgba: RefCell<Vec<Srgba8>>,
    error: RefCell<Option<FillError>>,
}

#[derive(Clone)]
pub struct FillGroup(Rc<Group>);

impl fmt::Debug for FillGroup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FillGroup")
            .field("nodes", &self.0.nodes.len())
            .finish()
    }
}

impl FillGroup {
    pub fn new(tree: &UiTree, nodes: Vec<NodeId>) -> Result<Self, FillError> {
        let mut seen = HashSet::with_capacity(nodes.len());
        for &node in &nodes {
            if tree.get(node).is_none() {
                return Err(FillError::StaleNode(node));
            }
            if !seen.insert(node) {
                return Err(FillError::DuplicateNode(node));
            }
        }
        Ok(Self(Rc::new(Group {
            owner: Rc::clone(&tree.fill_owner),
            epoch: Cell::new(tree.structure_epoch()),
            recycled: RefCell::new(Vec::new()),
            recycled_srgba: RefCell::new(Vec::new()),
            error: RefCell::new(None),
            nodes,
        })))
    }

    pub fn len(&self) -> usize {
        self.0.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.nodes.is_empty()
    }

    pub fn take_error(&self) -> Option<FillError> {
        self.0.error.borrow_mut().take()
    }

    pub fn apply(&self, tree: &mut UiTree, colors: &[Oklch]) -> Result<(), FillError> {
        if !Rc::ptr_eq(&self.0.owner, &tree.fill_owner) {
            return Err(FillError::ForeignTree);
        }
        if colors.len() != self.len() {
            return Err(FillError::Count {
                expected: self.len(),
                actual: colors.len(),
            });
        }
        for (index, color) in colors.iter().enumerate() {
            if ![color.l, color.c, color.h, color.alpha]
                .iter()
                .all(|value| value.is_finite())
            {
                return Err(FillError::InvalidColor { index });
            }
        }
        let epoch = tree.structure_epoch();
        if self.0.epoch.get() != epoch {
            for &node in &self.0.nodes {
                if tree.get(node).is_none() {
                    return Err(FillError::StaleNode(node));
                }
            }
            self.0.epoch.set(epoch);
        }
        tree.set_fills(&self.0.nodes, colors);
        Ok(())
    }

    pub fn apply_srgba(&self, tree: &mut UiTree, colors: &[Srgba8]) -> Result<(), FillError> {
        if !Rc::ptr_eq(&self.0.owner, &tree.fill_owner) {
            return Err(FillError::ForeignTree);
        }
        if colors.len() != self.len() {
            return Err(FillError::Count {
                expected: self.len(),
                actual: colors.len(),
            });
        }
        let epoch = tree.structure_epoch();
        if self.0.epoch.get() != epoch {
            for &node in &self.0.nodes {
                if tree.get(node).is_none() {
                    return Err(FillError::StaleNode(node));
                }
            }
            self.0.epoch.set(epoch);
        }
        tree.set_srgba_fills(&self.0.nodes, colors);
        Ok(())
    }

    pub fn bind_srgba<D, F>(
        &self,
        scope: &Scope,
        sink: &CommandSink,
        deps: D,
        mut render: F,
    ) -> EffectHandle
    where
        D: DepTuple,
        F: FnMut(D::Values, &mut Vec<Srgba8>) + 'static,
    {
        let group = self.clone();
        let sink = sink.clone();
        let declared = deps.clone();
        scope.effect(deps, move |values| {
            let mut colors = std::mem::take(&mut *group.0.recycled_srgba.borrow_mut());
            colors.clear();
            aqua_reactive::read_check::render_scope(&declared, || render(values, &mut colors));
            sink.push(UiCommand::Fills(FillUpdate {
                group: group.clone(),
                values: FillValues::Srgba(colors),
            }));
        })
    }

    pub fn bind<D, F>(
        &self,
        scope: &Scope,
        sink: &CommandSink,
        deps: D,
        mut render: F,
    ) -> EffectHandle
    where
        D: DepTuple,
        F: FnMut(D::Values, &mut Vec<Oklch>) + 'static,
    {
        let group = self.clone();
        let sink = sink.clone();
        let declared = deps.clone();
        scope.effect(deps, move |values| {
            let mut colors = std::mem::take(&mut *group.0.recycled.borrow_mut());
            colors.clear();
            aqua_reactive::read_check::render_scope(&declared, || render(values, &mut colors));
            sink.push(UiCommand::Fills(FillUpdate {
                group: group.clone(),
                values: FillValues::Oklch(colors),
            }));
        })
    }
}

enum FillValues {
    Oklch(Vec<Oklch>),
    Srgba(Vec<Srgba8>),
}

pub struct FillUpdate {
    group: FillGroup,
    values: FillValues,
}

impl fmt::Debug for FillUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FillUpdate")
            .field("nodes", &self.group.len())
            .field(
                "colors",
                &match &self.values {
                    FillValues::Oklch(colors) => colors.len(),
                    FillValues::Srgba(colors) => colors.len(),
                },
            )
            .finish()
    }
}

impl FillUpdate {
    pub(crate) fn apply(self, tree: &mut UiTree) {
        let Self { group, values } = self;
        match values {
            FillValues::Oklch(mut colors) => {
                if let Err(error) = group.apply(tree, &colors) {
                    *group.0.error.borrow_mut() = Some(error);
                }
                colors.clear();
                let mut recycled = group.0.recycled.borrow_mut();
                if colors.capacity() > recycled.capacity() {
                    *recycled = colors;
                }
            }
            FillValues::Srgba(mut colors) => {
                if let Err(error) = group.apply_srgba(tree, &colors) {
                    *group.0.error.borrow_mut() = Some(error);
                }
                colors.clear();
                let mut recycled = group.0.recycled_srgba.borrow_mut();
                if colors.capacity() > recycled.capacity() {
                    *recycled = colors;
                }
            }
        }
    }
}
