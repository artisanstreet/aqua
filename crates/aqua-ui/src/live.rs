use std::cell::{Cell, RefCell};
use std::rc::Rc;

use aqua_core::{NodeId, Size};
use aqua_reactive::collection::{Collection, Query};
use aqua_reactive::{ReadSignal, Scope};

use crate::binding::View;
use crate::collection_runtime::{
    CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
};
use crate::element::{lower_specs_in, Element, Key, TypoChain};
use crate::layout::TextMeasurer;
use crate::program::MountBinding;
use crate::region::MountedRegion;
use crate::style::Style;
use crate::tree::UiTree;

type Row<K, T> = Rc<dyn Fn(&Scope, K, ReadSignal<Rc<T>>) -> Element>;

pub struct LiveList<K, T> {
    collection: Collection<K, T>,
    structure: ReadSignal<u64>,
    keys: Rc<dyn Fn() -> Vec<K>>,
    row: Row<K, T>,
    styles: Vec<Style>,
    row_height: f32,
    overscan: usize,
    virtualized: bool,
}

impl<K: Ord + Clone + Into<Key> + 'static, T: 'static> LiveList<K, T> {
    pub fn new(
        collection: Collection<K, T>,
        row: impl Fn(&Scope, K, ReadSignal<Rc<T>>) -> Element + 'static,
    ) -> Self {
        let source = collection.clone();
        let structure = collection.structure();
        Self {
            collection,
            structure,
            keys: Rc::new(move || source.keys()),
            row: Rc::new(row),
            styles: Vec::new(),
            row_height: 32.0,
            overscan: 4,
            virtualized: true,
        }
    }
    pub fn query<S: Ord + Clone + 'static>(
        query: Query<K, T, S>,
        row: impl Fn(&Scope, K, ReadSignal<Rc<T>>) -> Element + 'static,
    ) -> Self {
        let collection = query.collection().clone();
        let structure = query.revision();
        Self {
            collection,
            structure,
            keys: Rc::new(move || query.keys()),
            row: Rc::new(row),
            styles: Vec::new(),
            row_height: 32.0,
            overscan: 4,
            virtualized: true,
        }
    }
    pub fn styles(mut self, styles: impl IntoIterator<Item = Style>) -> Self {
        self.styles = styles.into_iter().collect();
        self
    }
    pub fn estimated_row_height(mut self, height: f32) -> Self {
        self.row_height = height;
        self
    }
    pub fn overscan(mut self, rows: usize) -> Self {
        self.overscan = rows;
        self
    }
    pub fn mount_all(mut self) -> Self {
        self.virtualized = false;
        self
    }
    pub fn build(self) -> Element {
        crate::element::ui::Box::new()
            .style(crate::style::Style::new().opacity(1.0))
            .build()
            .install(self)
    }
}

impl<K: Ord + Clone + Into<Key> + 'static, T: 'static> MountBinding for LiveList<K, T> {
    fn owns_children(&self) -> bool {
        true
    }
    fn dependencies(&self) -> Vec<aqua_reactive::NodeId> {
        vec![self.structure.node_id()]
    }
    fn mount(&self, tree: &mut UiTree, node: NodeId, scope: &Scope, typography: &TypoChain) {
        let pending = Rc::new(Cell::new(false));
        let version = self.structure.version();
        tree.install_region(
            node,
            Box::new(MountedList {
                collection: self.collection.clone(),
                structure: self.structure.clone(),
                keys: self.keys.clone(),
                row: self.row.clone(),
                styles: self.styles.clone(),
                row_height: self.row_height,
                overscan: self.overscan,
                virtualized: self.virtualized,
                owner: scope.clone(),
                typography: typography.clone(),
                parent: node,
                view: None,
                revision: None,
                pending: pending.clone(),
            }),
        );
        crate::region::invalidate(tree, node, scope, &self.structure, pending, version);
    }
}

struct Source<K, T> {
    collection: Collection<K, T>,
    keys: Vec<K>,
    incarnations: Vec<u64>,
    row: Row<K, T>,
    owner: Scope,
}

impl<K: Ord + Clone + Into<Key> + 'static, T: 'static> CollectionSource for Source<K, T> {
    fn len(&self) -> usize {
        self.keys.len()
    }
    fn key(&self, index: usize) -> Key {
        self.keys[index].clone().into()
    }
    fn row_revision(&self, index: usize) -> Option<u64> {
        self.incarnations.get(index).copied()
    }
    fn layout(&self, width: f32) -> CollectionLayout {
        CollectionLayout {
            content_width: width,
            column_widths: Vec::new(),
        }
    }
    fn row(&self, index: usize, _: &CollectionLayout) -> Element {
        let owner = self.owner.child();
        let key = self.keys[index].clone();
        let row = self
            .collection
            .observe(&owner, &key)
            .expect("live row must exist during realization");
        (self.row)(&owner, key, row).install(Lease {
            scope: RefCell::new(Some(owner)),
        })
    }
    fn header(&self, _: &CollectionLayout) -> Option<Element> {
        None
    }
    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }
    fn validate(&self) -> Result<(), CollectionError> {
        Ok(())
    }
}

struct Lease {
    scope: RefCell<Option<Scope>>,
}

impl MountBinding for Lease {
    fn dependencies(&self) -> Vec<aqua_reactive::NodeId> {
        Vec::new()
    }
    fn mount(&self, _: &mut UiTree, _: NodeId, scope: &Scope, _: &TypoChain) {
        if let Some(owner) = self.scope.borrow_mut().take() {
            scope.on_cleanup(move || owner.dispose());
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(scope) = self.scope.get_mut().take() {
            scope.dispose();
        }
    }
}

struct MountedList<K, T> {
    collection: Collection<K, T>,
    structure: ReadSignal<u64>,
    keys: Rc<dyn Fn() -> Vec<K>>,
    row: Row<K, T>,
    styles: Vec<Style>,
    row_height: f32,
    overscan: usize,
    virtualized: bool,
    owner: Scope,
    typography: TypoChain,
    parent: NodeId,
    view: Option<View>,
    revision: Option<u64>,
    pending: Rc<Cell<bool>>,
}

impl<K: Ord + Clone + Into<Key> + 'static, T: 'static> MountedRegion for MountedList<K, T> {
    fn needs_layout(&self) -> bool {
        true
    }
    fn update(&mut self, tree: &mut UiTree) -> Result<(), String> {
        self.pending.set(false);
        let Some(revision) = self.structure.try_get() else {
            return Ok(());
        };
        if self.revision == Some(revision) {
            return Ok(());
        }
        let keys = (self.keys)();
        let incarnations = keys
            .iter()
            .map(|key| {
                self.collection
                    .incarnation(key)
                    .expect("live collection keys must exist")
            })
            .collect();
        let descriptor = CollectionDescriptor::new(
            Rc::new(Source {
                collection: self.collection.clone(),
                keys,
                incarnations,
                row: self.row.clone(),
                owner: self.owner.clone(),
            }),
            self.styles.clone(),
            self.virtualized,
            self.row_height,
            self.overscan,
        );
        let element = Element::collection(descriptor);
        match &mut self.view {
            Some(view) => {
                view.set_content(tree, vec![element])
                    .map_err(|error| error.to_string())?;
            }
            None => {
                let mut specs = lower_specs_in(&element, &self.typography)
                    .map_err(|error| error.to_string())?;
                let node = tree
                    .insert_spec(self.parent, 0, specs.remove(0))
                    .ok_or("live list owner was removed")?;
                let mut view = View::new(self.owner.child(), tree.command_sink());
                view.bind_existing_in(tree, node, element, &self.typography)
                    .map_err(|error| error.to_string())?;
                self.view = Some(view);
            }
        }
        self.revision = Some(revision);
        Ok(())
    }
    fn layout(
        &mut self,
        tree: &mut UiTree,
        viewport: Size,
        measurer: &dyn TextMeasurer,
    ) -> Result<(), String> {
        match &mut self.view {
            Some(view) => view.compute_layout(tree, viewport, measurer),
            None => Ok(()),
        }
    }
}

impl<K, T> Drop for MountedList<K, T> {
    fn drop(&mut self) {
        if let Some(view) = self.view.take() {
            view.dispose();
        }
    }
}
