//! Typed signal handles and explicit dependency tuples.
//!
//! There is deliberately **no implicit subscription**: reading a
//! [`ReadSignal`] never registers a dependency. A computation observes exactly
//! the handles passed to it at creation, so extracting code into a helper
//! cannot change the dependency set.

use std::cell::RefCell;
use std::rc::Rc;

use crate::graph::{NodeId, Slot};
use crate::runtime::{GraphError, GraphRef};

/// Typed value storage for one source or derived node.
pub(crate) struct ValueCell<T> {
    pub value: T,
    pub version: u64,
    pub disposed: bool,
}

/// Read-only handle to a reactive value.
///
/// Cloning shares the handle; every clone observes the same node. Reads never
/// subscribe — the only way to observe a value is to hold its handle, and the
/// only way to depend on it is to pass the handle explicitly to
/// [`Scope::derive`](crate::Scope::derive),
/// [`Scope::effect`](crate::Scope::effect), or
/// [`Scope::select`](crate::Scope::select).
pub struct ReadSignal<T> {
    pub(crate) graph: GraphRef,
    pub(crate) node: NodeId,
    pub(crate) slot: Slot,
    pub(crate) cell: Rc<RefCell<ValueCell<T>>>,
}

impl<T> Clone for ReadSignal<T> {
    fn clone(&self) -> Self {
        Self {
            graph: self.graph.clone(),
            node: self.node,
            slot: self.slot,
            cell: self.cell.clone(),
        }
    }
}

impl<T> ReadSignal<T> {
    /// This handle's node identity, for diagnostics and inspector output.
    pub fn node_id(&self) -> NodeId {
        self.node
    }

    /// True when the runtime and this node are both still alive.
    pub fn is_live(&self) -> bool {
        self.try_version().is_some()
    }

    /// Current version counter; advanced on every committed change.
    /// Returns `None` when the runtime was dropped or the node disposed.
    pub fn try_version(&self) -> Option<u64> {
        let inner = self.graph.inner.upgrade()?;
        let guard = inner.borrow();
        if guard.handle_live(self.node, self.slot) {
            Some(self.cell.borrow().version)
        } else {
            None
        }
    }

    /// Current version counter. Panics with a diagnostic when the handle is stale.
    pub fn version(&self) -> u64 {
        self.try_version().unwrap_or_else(|| {
            panic!(
                "aqua-reactive: version() on dead signal node {:?} \
                 (runtime dropped or scope disposed)",
                self.node
            )
        })
    }

    pub(crate) fn same_graph(&self, other: &GraphRef) -> bool {
        match (self.graph.inner.upgrade(), other.inner.upgrade()) {
            (Some(a), Some(b)) => Rc::ptr_eq(&a, &b),
            _ => false,
        }
    }
}

impl<T: Clone> ReadSignal<T> {
    /// Non-subscribing read. Returns `None` when the runtime was dropped or
    /// the node was disposed.
    ///
    /// Because Aqua has no implicit tracking, *every* read is a snapshot:
    /// calling this inside a computation does not add a dependency.
    pub fn try_get(&self) -> Option<T> {
        let inner = self.graph.inner.upgrade()?;
        let guard = inner.borrow();
        if !guard.handle_live(self.node, self.slot) {
            return None;
        }
        let cell = self.cell.borrow();
        if cell.disposed {
            None
        } else {
            Some(cell.value.clone())
        }
    }

    /// Non-subscribing read. Panics with a diagnostic when the handle is stale.
    pub fn get(&self) -> T {
        self.try_get().unwrap_or_else(|| {
            panic!(
                "aqua-reactive: get() on dead signal node {:?} \
                 (runtime dropped or scope disposed; use try_get() for fallible reads)",
                self.node
            )
        })
    }

    /// Alias for [`try_get`](Self::try_get) that makes snapshot intent explicit
    /// at call sites inside computations.
    pub fn snapshot(&self) -> Option<T> {
        self.try_get()
    }
}

/// Writable handle to a reactive source value.
///
/// Split from [`ReadSignal`] so authority to mutate is explicit: handing out a
/// `ReadSignal` never grants mutation authority, and a derived value never
/// produces a `WriteSignal`.
pub struct WriteSignal<T> {
    pub(crate) graph: GraphRef,
    pub(crate) node: NodeId,
    pub(crate) slot: Slot,
    pub(crate) cell: Rc<RefCell<ValueCell<T>>>,
}

impl<T> Clone for WriteSignal<T> {
    fn clone(&self) -> Self {
        Self {
            graph: self.graph.clone(),
            node: self.node,
            slot: self.slot,
            cell: self.cell.clone(),
        }
    }
}

impl<T: Clone + 'static> WriteSignal<T> {
    /// This handle's node identity, for diagnostics and inspector output.
    pub fn node_id(&self) -> NodeId {
        self.node
    }

    /// True when the runtime and this node are both still alive.
    pub fn is_live(&self) -> bool {
        self.graph
            .upgraded()
            .is_some_and(|inner| inner.borrow().handle_live(self.node, self.slot))
    }

    /// Set the value and propagate. Always notifies (no equality check);
    /// derived nodes apply their own equality suppression downstream.
    /// Panics with a diagnostic on a stale handle.
    pub fn set(&self, value: T) {
        if let Err(e) = self.try_set(value) {
            panic!("aqua-reactive: set() failed: {e:?}");
        }
    }

    /// Fallible [`set`](Self::set).
    ///
    /// Fails with [`GraphError::DerivedWrite`] when called while a derived
    /// computation is evaluating: derived values must be pure and read-only.
    pub fn try_set(&self, value: T) -> Result<(), GraphError> {
        let inner = self.graph.upgraded().ok_or(GraphError::RuntimeDropped)?;
        {
            let guard = inner.borrow();
            if !guard.handle_live(self.node, self.slot) {
                return Err(GraphError::Disposed(format!(
                    "write to dead signal node {:?}",
                    self.node
                )));
            }
            if guard.derive_depth > 0 {
                return Err(GraphError::DerivedWrite(format!(
                    "write to signal node {:?} rejected: derived computations are pure and read-only; \
                     move the write into an effect, an event handler, or setup code",
                    self.node
                )));
            }
        }
        {
            let mut cell = self.cell.borrow_mut();
            cell.value = value;
            cell.version = cell.version.wrapping_add(1);
        }
        self.graph.note_changed(self.node);
        Ok(())
    }

    /// Mutate in place and propagate. The closure runs briefly; it must not
    /// reenter the graph (reads/writes inside it will panic on re-borrow).
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        if let Err(e) = self.try_update(f) {
            panic!("aqua-reactive: update() failed: {e:?}");
        }
    }

    /// Fallible [`update`](Self::update). Same [`GraphError::DerivedWrite`]
    /// purity rule as [`try_set`](Self::try_set).
    pub fn try_update(&self, f: impl FnOnce(&mut T)) -> Result<(), GraphError> {
        let inner = self.graph.upgraded().ok_or(GraphError::RuntimeDropped)?;
        {
            let guard = inner.borrow();
            if !guard.handle_live(self.node, self.slot) {
                return Err(GraphError::Disposed(format!(
                    "update of dead signal node {:?}",
                    self.node
                )));
            }
            if guard.derive_depth > 0 {
                return Err(GraphError::DerivedWrite(format!(
                    "update of signal node {:?} rejected: derived computations are pure and read-only",
                    self.node
                )));
            }
        }
        {
            let mut cell = self.cell.borrow_mut();
            f(&mut cell.value);
            cell.version = cell.version.wrapping_add(1);
        }
        self.graph.note_changed(self.node);
        Ok(())
    }
}

impl<T: Clone + PartialEq + 'static> WriteSignal<T> {
    /// Set the value only when it differs from the current one. Returns `true`
    /// when a change was committed (and propagated).
    ///
    /// This is the equality-aware write; plain [`set`](Self::set) always
    /// notifies. NaN and large-collection costs are the caller's concern: NaN
    /// != NaN always notifies, and comparison cost is `PartialEq` cost.
    pub fn set_if_changed(&self, value: T) -> bool {
        let equal = match self.graph.upgraded() {
            Some(inner) => {
                let guard = inner.borrow();
                if !guard.handle_live(self.node, self.slot) {
                    panic!(
                        "aqua-reactive: set_if_changed() on dead signal node {:?}",
                        self.node
                    );
                }
                self.cell.borrow().value == value
            }
            None => panic!("aqua-reactive: set_if_changed() after Runtime was dropped"),
        };
        if equal {
            return false;
        }
        self.set(value);
        true
    }
}

/// Owned subscription to an effect. Dropping the handle does **not**
/// unsubscribe (the owner scope owns the subscription); call
/// [`dispose`](Self::dispose) or dispose the scope.
pub struct EffectHandle {
    pub(crate) graph: GraphRef,
    pub(crate) node: NodeId,
    pub(crate) slot: Slot,
}

impl EffectHandle {
    /// This handle's node identity, for diagnostics and inspector output.
    pub fn node_id(&self) -> NodeId {
        self.node
    }

    /// True when the runtime and this effect are both still alive.
    pub fn is_live(&self) -> bool {
        self.graph
            .upgraded()
            .is_some_and(|inner| inner.borrow().handle_live(self.node, self.slot))
    }

    /// Detach this effect: it will not run again. Queued entries are purged
    /// on dispose; any entry already popped is skipped by liveness check.
    pub fn dispose(self) {
        self.graph.dispose_node(self.node);
    }
}

// ---------------------------------------------------------------------------
// Explicit dependency tuples
// ---------------------------------------------------------------------------

/// A tuple of [`ReadSignal`] handles passed as the declared inputs of a
/// derived value or effect.
///
/// Implemented for 1- to 8-tuples. Single-handle convenience methods
/// (`derive1`, `effect1`, `subscribe`) cover the common case without tuple
/// syntax.
pub trait DepTuple: Clone + 'static {
    /// Cloned snapshot values, in tuple order.
    type Values;
    /// Node ids, in tuple order.
    fn ids(&self) -> Vec<NodeId>;
    /// Cloned values, in tuple order. Borrows are released before returning.
    fn snapshot(&self) -> Self::Values;
    /// False when any input cell is already disposed.
    fn all_live(&self) -> bool;
    /// Append current versions, in tuple order, reusing the caller's buffer
    /// (no per-read allocation once warmed).
    fn push_versions(&self, out: &mut Vec<u64>);
    /// True when `other` equals the current versions (same length and order).
    fn versions_match(&self, other: &[u64]) -> bool;
    /// Upgraded runtime identity pointers, one per input. Hidden seam that
    /// lets scopes reject cross-runtime mixing without exposing internals.
    #[doc(hidden)]
    fn __graph_ptrs(&self) -> Vec<*const ()>;
}

macro_rules! impl_dep_tuple {
    ( $($idx:tt $T:ident),+ ) => {
        impl<$($T: Clone + 'static),+> DepTuple for ($(ReadSignal<$T>,)+) {
            type Values = ($($T,)+);
            fn ids(&self) -> Vec<NodeId> {
                vec![$(self.$idx.node,)+]
            }
            fn push_versions(&self, out: &mut Vec<u64>) {
                $( out.push(self.$idx.cell.borrow().version); )+
            }
            fn versions_match(&self, other: &[u64]) -> bool {
                let mut i = 0;
                $(
                    if other.get(i) != Some(&self.$idx.cell.borrow().version) {
                        return false;
                    }
                    i += 1;
                )+
                i == other.len()
            }
            fn snapshot(&self) -> Self::Values {
                ($(self.$idx.cell.borrow().value.clone(),)+)
            }
            fn all_live(&self) -> bool {
                let mut ok = true;
                $( ok = ok && !self.$idx.cell.borrow().disposed; )+
                ok
            }
            fn __graph_ptrs(&self) -> Vec<*const ()> {
                vec![$( self.$idx.graph.inner.upgrade().map(|rc| Rc::as_ptr(&rc) as *const ()).unwrap_or(std::ptr::null()), )+]
            }
        }
    };
}

impl_dep_tuple!(0 A);
impl_dep_tuple!(0 A, 1 B);
impl_dep_tuple!(0 A, 1 B, 2 C);
impl_dep_tuple!(0 A, 1 B, 2 C, 3 D);
impl_dep_tuple!(0 A, 1 B, 2 C, 3 D, 4 E);
impl_dep_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F);
impl_dep_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G);
impl_dep_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H);
