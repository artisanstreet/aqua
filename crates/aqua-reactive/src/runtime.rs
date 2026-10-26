//! [`Runtime`] and [`Scope`]: ownership, propagation, batching, and effects.
//!
//! Ownership: [`Runtime`] holds the only strong references to shared state
//! (topology, thunks, edge ops, config). Every handle and scope holds a
//! [`GraphRef`] of [`Weak`] pointers only, so dropping the `Runtime` (and all
//! `Scope`s) frees the graph even while stale handles exist — stale use is
//! diagnosed, and reference cycles are structurally impossible.
//!
//! Borrow discipline (case-study requirements 6, 10, 35): user callbacks
//! (derive/effect/cleanup/resolve functions) **never** execute while an
//! [`Inner`](crate::graph::Inner) borrow is held. Thunks are moved out of the
//! side table into a local, all borrows are released, and only then is user
//! code invoked. Panics therefore unwind through clean state; bookkeeping is
//! restored before the panic continues.
//!
//! Purity: derived computations must be read-only. While a derive thunk runs,
//! [`Inner::derive_depth`] is nonzero and every signal write is rejected with
//! [`GraphError::DerivedWrite`] — otherwise the write could neither observe
//! nor trigger the temporarily-removed thunk coherently. Effects *may* write;
//! those writes queue further effects bounded by the flush limit.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{resume_unwind, AssertUnwindSafe};
use std::rc::{Rc, Weak};

use crate::graph::{EdgeOp, Inner, NodeId, NodeKind, ScopeId, Slot};
use crate::resource::{Resource, ResourceShared, ResourceState};
use crate::signals::{DepTuple, EffectHandle, ReadSignal, ValueCell, WriteSignal};

/// Why a fallible operation failed. Creation APIs (`signal`, `derive`,
/// `effect`, `select`) panic with a diagnostic instead: wiring a dead scope
/// or a foreign runtime's handle is a programming error, fail fast. Fallible
/// paths exist where liveness is legitimately racy (worker completion,
/// runtime teardown): `try_get`, `try_set`, `try_update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    /// The node or scope was disposed. Carries a diagnostic message.
    Disposed(String),
    /// The owning [`Runtime`] was dropped; weak handles no longer upgrade.
    RuntimeDropped,
    /// A dependency edge that would close a cycle was rejected.
    Cycle(Vec<NodeId>),
    /// Handles from two different runtimes were mixed (rejected).
    CrossRuntime,
    /// A signal write was attempted while a derived computation was
    /// evaluating. Derived values must be pure and read-only; move the write
    /// into an effect, an event handler, or ordinary setup code.
    DerivedWrite(String),
}

/// Outcome of [`Runtime::flush`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushReport {
    /// Effects whose callbacks actually ran.
    pub ran: usize,
    /// Queued effects skipped (disposed, inputs unchanged, or no thunk).
    pub skipped: usize,
    /// True when the iteration bound stopped the flush; remaining work stays
    /// queued. Indicates a feedback loop, not a silent drop.
    pub truncated: bool,
    /// True when `flush` was called reentrantly from inside an effect; the
    /// outer flush owns the queue and the inner call did nothing.
    pub reentered: bool,
}

/// Point-in-time inspector counts. Reading stats never mutates the graph, so
/// inspection cannot change update results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphStats {
    pub sources: usize,
    pub derived: usize,
    pub effects: usize,
    pub edges: usize,
    pub live_scopes: usize,
    pub queued_effects: usize,
    pub cycle_rejections: u64,
    pub flush_truncations: u64,
    /// Completions rejected as stale across all resources.
    pub stale_completions: u64,
}

/// Strong-reference counts for leak auditing. Handles and scopes hold only
/// [`Weak`] pointers, so after the `Runtime` and all user values/closures
/// are dropped these must all reach zero — a nonzero count after teardown
/// proves a retention bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retention {
    pub topology: usize,
    pub thunks: usize,
    pub edge_ops: usize,
    pub config: usize,
}

pub(crate) struct Config {
    pub auto_flush: bool,
    pub flush_limit: usize,
    pub wake: Option<Box<dyn Fn()>>,
}

/// User callbacks live here, keyed by node — never inside [`Inner`], so they
/// can be executed without holding any graph borrow.
pub(crate) enum Thunk {
    Derive(Box<dyn FnMut() -> bool>),
    Effect(Box<dyn FnMut() -> bool>),
}

/// The four live shared tables, upgraded together or not at all.
type Upgraded = (
    Rc<RefCell<Inner>>,
    Rc<RefCell<HashMap<NodeId, Thunk>>>,
    Rc<RefCell<Vec<EdgeOp>>>,
    Rc<RefCell<Config>>,
);

/// Strong shared state, owned exclusively by [`Runtime`].
#[derive(Clone)]
struct Shared {
    inner: Rc<RefCell<Inner>>,
    thunks: Rc<RefCell<HashMap<NodeId, Thunk>>>,
    edge_ops: Rc<RefCell<Vec<EdgeOp>>>,
    config: Rc<RefCell<Config>>,
}

/// Weak back-reference held by every signal handle, effect handle, and scope.
///
/// Upgrading fails once the owning [`Runtime`] is dropped; every access maps
/// that to [`GraphError::RuntimeDropped`] (or a diagnostic panic for
/// programmer-error paths) instead of use-after-free.
#[derive(Clone)]
pub(crate) struct GraphRef {
    pub(crate) inner: Weak<RefCell<Inner>>,
    pub(crate) thunks: Weak<RefCell<HashMap<NodeId, Thunk>>>,
    pub(crate) edge_ops: Weak<RefCell<Vec<EdgeOp>>>,
    pub(crate) config: Weak<RefCell<Config>>,
}

impl GraphRef {
    pub fn upgraded(&self) -> Option<Rc<RefCell<Inner>>> {
        self.inner.upgrade()
    }

    fn upgrade_all(&self) -> Option<Upgraded> {
        Some((
            self.inner.upgrade()?,
            self.thunks.upgrade()?,
            self.edge_ops.upgrade()?,
            self.config.upgrade()?,
        ))
    }

    /// Rebuild a [`Runtime`] while shared state is alive (used by scopes to
    /// run batch/flush against the live graph).
    fn runtime(&self) -> Option<Runtime> {
        let (inner, thunks, edge_ops, config) = self.upgrade_all()?;
        Some(Runtime {
            shared: Shared {
                inner,
                thunks,
                edge_ops,
                config,
            },
        })
    }

    fn scope_live(&self, scope: ScopeId) -> bool {
        self.upgraded().is_some_and(|inner| {
            inner
                .borrow()
                .scopes
                .get(&scope)
                .is_some_and(|s| !s.disposed)
        })
    }

    fn ensure_scope(&self, scope: ScopeId, what: &str) {
        if self.upgraded().is_none() {
            panic!("aqua-reactive: {what} after Runtime was dropped");
        }
        if !self.scope_live(scope) {
            panic!("aqua-reactive: {what} on a disposed scope");
        }
    }

    /// Eager, glitch-free propagation: recompute every dependent derive in
    /// topological order, then queue dependent effects. Called after every
    /// committed source write and after every effect-driven write.
    pub fn note_changed(&self, start: NodeId) {
        let Some((inner, _, _, _)) = self.upgrade_all() else {
            return;
        };
        let mut order = inner.borrow_mut().take_order_buffer();
        inner.borrow_mut().topo_into(start, &mut order);
        for slot in order.iter().copied() {
            self.run_derive(slot);
            self.drain_edge_ops();
        }
        let mut newly_queued = 0usize;
        {
            let mut guard = inner.borrow_mut();
            for slot in &order {
                if !matches!(guard.kind_of(*slot), Some(NodeKind::Effect { .. })) {
                    continue;
                }
                let already = matches!(
                    guard.kind_of(*slot),
                    Some(NodeKind::Effect { queued: true })
                );
                if already {
                    continue;
                }
                guard.set_queued(*slot, true);
                if let Some(id) = guard.id_of(*slot) {
                    guard.queue.push_back(id);
                    newly_queued += 1;
                }
            }
        }
        drop(inner);
        self.maybe_flush_or_wake(newly_queued);
        if let Some(inner) = self.upgraded() {
            inner.borrow_mut().reclaim_order(order);
        }
    }

    /// Run user code with the derivation purity guard held. Used for initial
    /// computations (which run before any node exists) as well as thunks:
    /// without this, a write during setup-time derivation would succeed.
    fn guarded_eval<R>(&self, f: impl FnOnce() -> R) -> R {
        let inner = self.upgraded();
        if let Some(ref i) = inner {
            i.borrow_mut().derive_depth += 1;
        }
        let result = std::panic::catch_unwind(AssertUnwindSafe(f));
        if let Some(ref i) = inner {
            i.borrow_mut().derive_depth -= 1;
        }
        match result {
            Ok(v) => v,
            Err(payload) => resume_unwind(payload),
        }
    }

    /// Run one derived node's recompute outside any graph borrow. Sets the
    /// purity guard so writes from derived computations are rejected. On
    /// panic, the thunk is restored and dependency bookkeeping is untouched
    /// (the thunk only commits its new versions after user code succeeds),
    /// then the panic continues.
    fn run_derive(&self, slot: Slot) {
        let Some((inner, thunks, _, _)) = self.upgrade_all() else {
            return;
        };
        let id = {
            let guard = inner.borrow();
            match guard.record(slot) {
                Some(rec) if matches!(rec.kind, NodeKind::Derived) => rec.id,
                _ => return,
            }
        };
        {
            let mut guard = inner.borrow_mut();
            guard.derive_depth += 1;
        }
        let thunk = thunks.borrow_mut().remove(&id);
        let mut thunk = match thunk {
            Some(Thunk::Derive(f)) => f,
            Some(other) => {
                thunks.borrow_mut().insert(id, other);
                inner.borrow_mut().derive_depth -= 1;
                return;
            }
            None => {
                inner.borrow_mut().derive_depth -= 1;
                return;
            }
        };
        let result = std::panic::catch_unwind(AssertUnwindSafe(&mut thunk));
        // Restore only when the slot still names this node as a live derive:
        // disposal — or slot recycling — drops the thunk instead.
        let still_live = inner.borrow().handle_live(id, slot)
            && matches!(inner.borrow().kind_of(slot), Some(NodeKind::Derived));
        if still_live {
            thunks.borrow_mut().insert(id, Thunk::Derive(thunk));
        }
        // If the node was disposed mid-computation the thunk is dropped.
        inner.borrow_mut().derive_depth -= 1;
        if let Err(payload) = result {
            resume_unwind(payload);
        }
    }

    /// Apply deferred dynamic re-subscriptions with cycle checking.
    fn drain_edge_ops(&self) {
        let Some((inner, _, edge_ops, _)) = self.upgrade_all() else {
            return;
        };
        let ops: Vec<EdgeOp> = edge_ops.borrow_mut().drain(..).collect();
        if ops.is_empty() {
            return;
        }
        let mut guard = inner.borrow_mut();
        for op in ops {
            match op {
                EdgeOp::Relink { node, remove, add } => {
                    guard.apply_relink(node, &remove, &add);
                }
            }
        }
    }

    fn maybe_flush_or_wake(&self, newly_queued: usize) {
        if newly_queued == 0 {
            return;
        }
        let Some((inner, _, _, config)) = self.upgrade_all() else {
            return;
        };
        enum Action {
            Flush,
            Wake,
            None,
        }
        let action = {
            let guard = inner.borrow();
            if guard.batch_depth > 0 || guard.flushing {
                Action::None
            } else {
                let cfg = config.borrow();
                if cfg.auto_flush {
                    Action::Flush
                } else if cfg.wake.is_some() {
                    Action::Wake
                } else {
                    Action::None
                }
            }
        };
        match action {
            Action::Flush => {
                drop(inner);
                if let Some(rt) = self.runtime() {
                    rt.flush();
                }
            }
            Action::Wake => {
                let cb = config.borrow_mut().wake.take();
                if let Some(cb) = cb {
                    cb();
                    config.borrow_mut().wake = Some(cb);
                }
            }
            Action::None => {}
        }
    }

    /// Dispose one node and cascade to its dependents. Disposal detaches
    /// edges, marks typed cells, drops thunks, purges queued entries, and
    /// **removes map entries** — no tombstone accumulation under keyed-list
    /// churn. Dependent nodes are disposed too: a computation over a dead
    /// input cannot produce meaningful values, and silent freezing is worse
    /// than an explicit, diagnosable death.
    pub fn dispose_node(&self, id: NodeId) {
        let Some((inner, thunks, _, _)) = self.upgrade_all() else {
            return;
        };
        let mut stack = vec![id];
        let mut drop_thunks: Vec<Thunk> = Vec::new();
        let mut drop_disposers: Vec<Box<dyn Fn()>> = Vec::new();
        {
            let mut guard = inner.borrow_mut();
            while let Some(cur) = stack.pop() {
                let Some(det) = guard.detach_node(cur) else {
                    continue;
                };
                stack.extend(det.dependents);
                if let Some(t) = thunks.borrow_mut().remove(&cur) {
                    drop_thunks.push(t);
                }
                if let Some(d) = det.on_dispose {
                    drop_disposers.push(d);
                }
            }
        }
        // User `Drop` impls and cleanups run with no graph borrow held
        // (they must still not reenter the graph).
        for d in drop_disposers {
            d();
        }
        drop(drop_thunks);
    }
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// Owner of one explicit dependency graph. Holds the only strong references
/// to shared state; cheap to clone (shares the graph). The graph lives until
/// the last `Runtime` is dropped — handles and scopes hold [`Weak`]
/// references only, so a dropped runtime is diagnosed via
/// [`GraphError::RuntimeDropped`], never use-after-free.
///
/// Single-threaded by design for the initial UI executor: handles are
/// `!Send + !Sync`. Cross-thread handoff goes through messages at the
/// facade layer, never by sharing handles.
#[derive(Clone)]
pub struct Runtime {
    shared: Shared,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            shared: Shared {
                inner: Rc::new(RefCell::new(Inner::new())),
                thunks: Rc::new(RefCell::new(HashMap::new())),
                edge_ops: Rc::new(RefCell::new(Vec::new())),
                config: Rc::new(RefCell::new(Config {
                    auto_flush: true,
                    flush_limit: 1024,
                    wake: None,
                })),
            },
        }
    }

    fn graph_ref(&self) -> GraphRef {
        GraphRef {
            inner: Rc::downgrade(&self.shared.inner),
            thunks: Rc::downgrade(&self.shared.thunks),
            edge_ops: Rc::downgrade(&self.shared.edge_ops),
            config: Rc::downgrade(&self.shared.config),
        }
    }

    /// Create a top-level owner scope.
    pub fn create_scope(&self) -> Scope {
        let mut guard = self.shared.inner.borrow_mut();
        let id: ScopeId = guard.next_scope;
        guard.next_scope += 1;
        guard.scopes.insert(
            id,
            crate::graph::ScopeRecord {
                parent: None,
                children: Vec::new(),
                members: Vec::new(),
                cleanups: Vec::new(),
                disposed: false,
            },
        );
        Scope {
            graph: self.graph_ref(),
            id,
        }
    }

    /// Coalescing batch: derived values stay fresh on read, but effects queued
    /// inside run once with the latest values. Batches nest; only the
    /// outermost boundary flushes. A batch is **not** a transaction: writes
    /// are committed immediately, and a panic does not roll them back — it
    /// truncates effects queued by the failed batch and restores depth so the
    /// graph stays coherent.
    pub fn batch<R>(&self, f: impl FnOnce() -> R) -> R {
        {
            let mut guard = self.shared.inner.borrow_mut();
            guard.batch_depth += 1;
            let mark = guard.queue.len();
            guard.batch_marks.push(mark);
        }
        let mut guard = BatchGuard {
            graph: self.graph_ref(),
            committed: false,
        };
        let result = f();
        guard.committed = true;
        drop(guard);
        result
    }

    /// Run all queued effects. Effects queued by effects also run, in queue
    /// order, until the queue drains or the flush limit is reached.
    /// Reentrant `flush` from inside an effect is a documented no-op
    /// (`reentered: true`).
    pub fn flush(&self) -> FlushReport {
        {
            let mut guard = self.shared.inner.borrow_mut();
            if guard.flushing {
                return FlushReport {
                    ran: 0,
                    skipped: 0,
                    truncated: false,
                    reentered: true,
                };
            }
            guard.flushing = true;
        }
        let _reset = FlushGuard {
            inner: Rc::downgrade(&self.shared.inner),
        };
        let limit = self.shared.config.borrow().flush_limit;
        let mut report = FlushReport {
            ran: 0,
            skipped: 0,
            truncated: false,
            reentered: false,
        };
        loop {
            if report.ran + report.skipped >= limit {
                if !self.shared.inner.borrow().queue.is_empty() {
                    report.truncated = true;
                    self.shared.inner.borrow_mut().flush_truncations += 1;
                }
                break;
            }
            let next: Option<(NodeId, Slot)> = {
                let mut guard = self.shared.inner.borrow_mut();
                loop {
                    let Some(id) = guard.queue.pop_front() else {
                        break None;
                    };
                    let Some(slot) = guard.slot_of(id) else {
                        report.skipped += 1;
                        continue;
                    };
                    if !guard.handle_live(id, slot) {
                        report.skipped += 1;
                        continue;
                    }
                    guard.set_queued(slot, false);
                    break Some((id, slot));
                }
            };
            let Some((id, slot)) = next else { break };
            let live = self.shared.inner.borrow().handle_live(id, slot)
                && matches!(
                    self.shared.inner.borrow().kind_of(slot),
                    Some(NodeKind::Effect { .. })
                );
            if !live {
                report.skipped += 1;
                continue;
            }
            let thunk = self.shared.thunks.borrow_mut().remove(&id);
            let mut thunk = match thunk {
                Some(Thunk::Effect(f)) => f,
                Some(other) => {
                    self.shared.thunks.borrow_mut().insert(id, other);
                    report.skipped += 1;
                    continue;
                }
                None => {
                    report.skipped += 1;
                    continue;
                }
            };
            let result = std::panic::catch_unwind(AssertUnwindSafe(&mut thunk));
            // Restore before observing the outcome: a disposed-during-run
            // node — or a recycled slot — drops the thunk instead.
            let still_live = self.shared.inner.borrow().handle_live(id, slot)
                && matches!(
                    self.shared.inner.borrow().kind_of(slot),
                    Some(NodeKind::Effect { .. })
                );
            if still_live {
                self.shared
                    .thunks
                    .borrow_mut()
                    .insert(id, Thunk::Effect(thunk));
            }
            match result {
                Ok(true) => report.ran += 1,
                Ok(false) => report.skipped += 1,
                Err(payload) => {
                    // The flushing flag is reset by `_reset`; the remaining
                    // queue is preserved for a later flush. Versions were only
                    // committed after user code succeeded, so the failed
                    // effect retries cleanly.
                    resume_unwind(payload);
                }
            }
        }
        report
    }

    /// Number of effects currently queued.
    pub fn pending(&self) -> usize {
        self.shared.inner.borrow().queue.len()
    }

    /// When true (default), writes flush effects synchronously at the
    /// outermost batch boundary. Set false in tests or when the host UI
    /// scheduler drives [`flush`](Self::flush) at update boundaries.
    pub fn set_auto_flush(&self, auto: bool) {
        self.shared.config.borrow_mut().auto_flush = auto;
    }

    /// Bound on effect executions per [`flush`](Self::flush). Exceeding it
    /// stops the flush with `truncated: true` instead of looping forever on
    /// a feedback cycle.
    pub fn set_flush_limit(&self, limit: usize) {
        self.shared.config.borrow_mut().flush_limit = limit.max(1);
    }

    /// Callback invoked when effects are queued while manual flushing is in
    /// effect (`auto_flush == false`, outside any batch). The UI scheduler
    /// uses this to schedule an update-boundary flush.
    pub fn set_wake_callback(&self, f: impl Fn() + 'static) {
        self.shared.config.borrow_mut().wake = Some(Box::new(f));
    }

    /// Point-in-time counts for the inspector. Read-only; no graph mutation.
    pub fn stats(&self) -> GraphStats {
        let guard = self.shared.inner.borrow();
        let mut stats = GraphStats {
            queued_effects: guard.queue.len(),
            cycle_rejections: guard.cycle_rejections,
            flush_truncations: guard.flush_truncations,
            stale_completions: guard.stale_completions,
            live_scopes: guard.scopes.values().filter(|s| !s.disposed).count(),
            ..Default::default()
        };
        for rec in guard.live_records() {
            stats.edges += rec.dependencies.len();
            match rec.kind {
                NodeKind::Source => stats.sources += 1,
                NodeKind::Derived => stats.derived += 1,
                NodeKind::Effect { .. } => stats.effects += 1,
            }
        }
        stats
    }

    /// Strong-reference counts for leak auditing (inspector support). All
    /// counts must reach zero after the `Runtime`, scopes, and user values
    /// are dropped; anything else is a retention bug.
    pub fn retention(&self) -> Retention {
        Retention {
            topology: Rc::strong_count(&self.shared.inner),
            thunks: Rc::strong_count(&self.shared.thunks),
            edge_ops: Rc::strong_count(&self.shared.edge_ops),
            config: Rc::strong_count(&self.shared.config),
        }
    }
}

struct BatchGuard {
    graph: GraphRef,
    committed: bool,
}

impl Drop for BatchGuard {
    fn drop(&mut self) {
        let Some(rt) = self.graph.runtime() else {
            return;
        };
        if self.committed {
            let (flush_needed, wake_needed) = {
                let mut guard = rt.shared.inner.borrow_mut();
                guard.batch_depth = guard.batch_depth.saturating_sub(1);
                guard.batch_marks.pop();
                if guard.batch_depth == 0 {
                    let cfg = rt.shared.config.borrow();
                    (cfg.auto_flush, !cfg.auto_flush && !guard.queue.is_empty())
                } else {
                    (false, false)
                }
            };
            if flush_needed {
                rt.flush();
            } else if wake_needed {
                let cb = rt.shared.config.borrow_mut().wake.take();
                if let Some(cb) = cb {
                    cb();
                    rt.shared.config.borrow_mut().wake = Some(cb);
                }
            }
        } else {
            // Unwind path: discard effects queued by the failed batch,
            // restore depth so outer batches and later updates stay coherent.
            let mut guard = rt.shared.inner.borrow_mut();
            guard.batch_depth = guard.batch_depth.saturating_sub(1);
            if let Some(mark) = guard.batch_marks.pop() {
                guard.truncate_queue(mark);
            }
        }
    }
}

struct FlushGuard {
    inner: Weak<RefCell<Inner>>,
}

impl Drop for FlushGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.borrow_mut().flushing = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Collected scope contents awaiting teardown (drops run unborrowed).
type Disposal = (Vec<NodeId>, Vec<u64>, Vec<Box<dyn FnOnce()>>);

/// Owner of signals, derived values, effects, and cleanups. Disposing a
/// scope disposes every member (cascading to their dependents), runs cleanups
/// LIFO, and recursively disposes child scopes.
///
/// A `Scope` holds only weak references: after the [`Runtime`] is dropped,
/// every method fails with a diagnostic instead of touching freed state.
#[derive(Clone)]
pub struct Scope {
    graph: GraphRef,
    id: ScopeId,
}

impl Scope {
    /// Scope identity, for diagnostics and inspector output.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// True when the runtime and this scope are both still alive.
    pub fn is_live(&self) -> bool {
        self.graph.scope_live(self.id)
    }

    /// Create a child scope. Disposing the child does not affect the parent;
    /// disposing the parent disposes the child.
    pub fn child(&self) -> Scope {
        self.graph.ensure_scope(self.id, "Scope::child");
        let inner = self.graph.upgraded().expect("checked above");
        let mut guard = inner.borrow_mut();
        let id: ScopeId = guard.next_scope;
        guard.next_scope += 1;
        if let Some(parent) = guard.scopes.get_mut(&self.id) {
            parent.children.push(id);
        }
        guard.scopes.insert(
            id,
            crate::graph::ScopeRecord {
                parent: Some(self.id),
                children: Vec::new(),
                members: Vec::new(),
                cleanups: Vec::new(),
                disposed: false,
            },
        );
        Scope {
            graph: self.graph.clone(),
            id,
        }
    }

    /// Register a cleanup run exactly once when this scope disposes (LIFO
    /// with other cleanups). Cleanups observe only snapshots: by the time
    /// they run, member nodes are already detached.
    pub fn on_cleanup(&self, f: impl FnOnce() + 'static) {
        self.graph.ensure_scope(self.id, "Scope::on_cleanup");
        if let Some(inner) = self.graph.upgraded() {
            if let Some(s) = inner.borrow_mut().scopes.get_mut(&self.id) {
                s.cleanups.push(Box::new(f));
            }
        }
    }

    /// Dispose this scope: cascade-dispose members, run cleanups, dispose
    /// children. Consumes the handle; clones observe `is_live() == false`.
    pub fn dispose(self) {
        self.dispose_ref();
    }

    fn dispose_ref(&self) {
        let Some(inner) = self.graph.upgraded() else {
            return;
        };
        // Collect first under short borrows; drops and user cleanups run
        // with no graph borrow held.
        let (members, children, cleanups): Disposal = {
            let mut guard = inner.borrow_mut();
            let Some(rec) = guard.scopes.get_mut(&self.id) else {
                return;
            };
            if rec.disposed {
                return;
            }
            rec.disposed = true;
            let mut cleanups = Vec::new();
            while let Some(c) = rec.cleanups.pop() {
                cleanups.push(c);
            }
            (
                std::mem::take(&mut rec.members),
                std::mem::take(&mut rec.children),
                cleanups,
            )
        };
        for child in children {
            Scope {
                graph: self.graph.clone(),
                id: child,
            }
            .dispose_ref();
        }
        for member in members {
            self.graph.dispose_node(member);
        }
        if let Some(inner) = self.graph.upgraded() {
            let mut guard = inner.borrow_mut();
            // Detach from parent.
            let parent = guard.scopes.get(&self.id).and_then(|s| s.parent);
            if let Some(p) = parent {
                if let Some(ps) = guard.scopes.get_mut(&p) {
                    ps.children.retain(|c| *c != self.id);
                }
            }
            guard.scopes.remove(&self.id);
        }
        for cleanup in cleanups {
            cleanup();
        }
    }

    /// Create a writable source signal with an initial value.
    pub fn signal<T: Clone + 'static>(&self, initial: T) -> (ReadSignal<T>, WriteSignal<T>) {
        self.graph.ensure_scope(self.id, "Scope::signal");
        let cell = Rc::new(RefCell::new(ValueCell {
            value: initial,
            version: 0,
            disposed: false,
        }));
        let cell_clone = cell.clone();
        let (node, slot) = self.alloc_member(
            NodeKind::Source,
            Some(Box::new(move || {
                cell_clone.borrow_mut().disposed = true;
            })),
        );
        (
            ReadSignal {
                graph: self.graph.clone(),
                node,
                slot,
                cell: cell.clone(),
            },
            WriteSignal {
                graph: self.graph.clone(),
                node,
                slot,
                cell,
            },
        )
    }

    /// Derived value over explicitly declared tuple inputs. Recomputes eagerly
    /// (topological order, glitch-free diamonds) when any input changes;
    /// downstream notification is suppressed when the new value equals the old
    /// (`PartialEq`). Requires `T: PartialEq`; see [`derive_no_eq`](Self::derive_no_eq)
    /// and [`derive_with`](Self::derive_with) otherwise.
    ///
    /// Derived computations are pure and read-only: signal writes inside `f`
    /// are rejected with [`GraphError::DerivedWrite`].
    ///
    /// The initial computation runs during creation, before the node is
    /// linked — a failure leaves no half-linked node behind.
    pub fn derive<D, T, F>(&self, deps: D, f: F) -> ReadSignal<T>
    where
        D: DepTuple,
        T: Clone + PartialEq + 'static,
        F: FnMut(D::Values) -> T + 'static,
    {
        self.derive_with(deps, f, PartialEq::eq)
    }

    /// Derived value without equality suppression: every recompute notifies.
    /// For values without meaningful equality, or where notification cost is
    /// preferred over comparison cost.
    pub fn derive_no_eq<D, T, F>(&self, deps: D, f: F) -> ReadSignal<T>
    where
        D: DepTuple,
        T: Clone + 'static,
        F: FnMut(D::Values) -> T + 'static,
    {
        self.derive_with(deps, f, |_, _| false)
    }

    /// Derived value with a custom equality function (float tolerance,
    /// structural comparison, interning checks).
    pub fn derive_with<D, T, F, E>(&self, deps: D, mut f: F, mut eq: E) -> ReadSignal<T>
    where
        D: DepTuple,
        T: Clone + 'static,
        F: FnMut(D::Values) -> T + 'static,
        E: FnMut(&T, &T) -> bool + 'static,
    {
        self.graph.ensure_scope(self.id, "Scope::derive");
        self.ensure_inputs(&deps, "Scope::derive");
        let initial = self.graph.guarded_eval(|| f(deps.snapshot()));
        let mut last = Vec::new();
        deps.push_versions(&mut last);
        let dep_ids = deps.ids();
        let cell = Rc::new(RefCell::new(ValueCell {
            value: initial,
            version: 0,
            disposed: false,
        }));
        let out = cell.clone();
        let mut scratch = Vec::new();
        let thunk = Thunk::Derive(Box::new(move || {
            if !deps.all_live() {
                return false;
            }
            scratch.clear();
            deps.push_versions(&mut scratch);
            if scratch == last {
                return false;
            }
            let values = deps.snapshot();
            let computed = f(values);
            // Commit versions only after user code succeeded: a panic
            // retries cleanly on the next trigger. Swap to reuse both
            // buffers' capacity across runs.
            std::mem::swap(&mut last, &mut scratch);
            let mut guard = out.borrow_mut();
            if eq(&guard.value, &computed) {
                return false;
            }
            guard.value = computed;
            guard.version = guard.version.wrapping_add(1);
            true
        }));
        let cell_clone = cell.clone();
        let (node, slot) = self.alloc_member(
            NodeKind::Derived,
            Some(Box::new(move || {
                cell_clone.borrow_mut().disposed = true;
            })),
        );
        if let Some((_, thunks, _, _)) = self.graph.upgrade_all() {
            thunks.borrow_mut().insert(node, thunk);
        }
        for dep in dep_ids {
            self.link(dep, node, "Scope::derive");
        }
        ReadSignal {
            graph: self.graph.clone(),
            node,
            slot,
            cell,
        }
    }

    /// Single-input [`derive`](Self::derive) without tuple syntax.
    pub fn derive1<A, T, F>(&self, a: &ReadSignal<A>, mut f: F) -> ReadSignal<T>
    where
        A: Clone + 'static,
        T: Clone + PartialEq + 'static,
        F: FnMut(A) -> T + 'static,
    {
        self.derive((a.clone(),), move |(a,)| f(a))
    }

    /// Fallible derivation: stores `Result<T, E>` so both success and failure
    /// are observable values with retry-on-input-change semantics. An `Err`
    /// propagates to dependents as a value (they can match on it); changing
    /// any input recomputes and may recover to `Ok`.
    pub fn try_derive<D, T, E, F>(&self, deps: D, f: F) -> ReadSignal<Result<T, E>>
    where
        D: DepTuple,
        T: Clone + PartialEq + 'static,
        E: Clone + PartialEq + 'static,
        F: FnMut(D::Values) -> Result<T, E> + 'static,
    {
        self.derive(deps, f)
    }

    /// Owned synchronization with external systems over declared inputs.
    /// Queued on creation (runs once initially) and after every input change;
    /// coalesced so multiple writes before a flush run the effect once with
    /// the latest values. May write signals; those writes queue further
    /// effects processed in the same flush, bounded by the flush limit.
    pub fn effect<D, F>(&self, deps: D, mut f: F) -> EffectHandle
    where
        D: DepTuple,
        F: FnMut(D::Values) + 'static,
    {
        self.graph.ensure_scope(self.id, "Scope::effect");
        self.ensure_inputs(&deps, "Scope::effect");
        let dep_ids = deps.ids();
        let mut last: Vec<u64> = Vec::new();
        let mut scratch: Vec<u64> = Vec::new();
        let mut armed = false;
        let thunk = Thunk::Effect(Box::new(move || {
            if !deps.all_live() {
                return false;
            }
            scratch.clear();
            deps.push_versions(&mut scratch);
            if armed && scratch == last {
                return false;
            }
            let values = deps.snapshot();
            f(values);
            std::mem::swap(&mut last, &mut scratch);
            armed = true;
            true
        }));
        let (node, slot) = self.alloc_member(NodeKind::Effect { queued: false }, None);
        for dep in dep_ids {
            self.link(dep, node, "Scope::effect");
        }
        let graph = self.graph.clone();
        if let Some((inner, thunks, _, _)) = graph.upgrade_all() {
            thunks.borrow_mut().insert(node, thunk);
            {
                let mut guard = inner.borrow_mut();
                guard.set_queued(slot, true);
                guard.queue.push_back(node);
            }
            drop(inner);
            graph.maybe_flush_or_wake(1);
        }
        EffectHandle { graph, node, slot }
    }

    /// Async resource with explicit inputs and generation-checked completion.
    ///
    /// `params_of` maps the declared input values to fetch parameters;
    /// whenever the inputs change, the resource advances its generation,
    /// publishes `Loading`, and queues a [`FetchRequest`](crate::FetchRequest)
    /// for the host executor (drain with `take_requests`, run on your
    /// runtime — see [`crate::worker`] — and commit back with
    /// [`Resource::complete`](crate::Resource::complete) on the owner thread).
    /// An initial fetch is queued on first flush, so resources load on mount.
    /// Stale completions (superseded, cancelled, duplicate, disposed) are
    /// rejected without touching state and counted in stats.
    ///
    /// The fetch itself never runs in the graph: no implicit async tracking,
    /// no speculative execution. Moving the call into a helper cannot change
    /// the dependency set.
    pub fn resource<D, T, E, P, F>(&self, deps: D, mut params_of: F) -> Resource<T, E, P>
    where
        D: DepTuple,
        T: Clone + PartialEq + 'static,
        E: Clone + PartialEq + 'static,
        P: Clone + 'static,
        F: FnMut(D::Values) -> P + 'static,
    {
        self.graph.ensure_scope(self.id, "Scope::resource");
        self.ensure_inputs(&deps, "Scope::resource");
        let (state_r, state_w) = self.signal(ResourceState::Loading { generation: 0 });
        let res = Resource::new(
            self.graph.clone(),
            state_r.node,
            state_r.slot,
            state_r,
            state_w,
            Rc::new(RefCell::new(ResourceShared::new())),
        );
        // Input-tracking effect: pure parameter mapping here; the async work
        // is the host's job. `track_request` only touches the outbox + state.
        let res_effect = res.clone();
        let _effect = self.effect(deps, move |values| {
            let params = params_of(values);
            res_effect.track_request(params);
        });
        res
    }

    /// Single-input [`effect`](Self::effect): the subscription primitive the
    /// UI layer uses to bind a value to a property or callback.
    pub fn effect1<A, F>(&self, a: &ReadSignal<A>, mut f: F) -> EffectHandle
    where
        A: Clone + 'static,
        F: FnMut(A) + 'static,
    {
        self.effect((a.clone(),), move |(a,)| f(a))
    }

    /// Alias for [`effect1`](Self::effect1) with binding intent: connect a
    /// source value to an external sink (UI property, logger, native API).
    pub fn subscribe<A, F>(&self, source: &ReadSignal<A>, f: F) -> EffectHandle
    where
        A: Clone + 'static,
        F: FnMut(A) + 'static,
    {
        self.effect1(source, f)
    }

    /// Explicit dynamic dependency: observes `key`, resolves the current
    /// target signal through `resolve`, and mirrors the target's value.
    /// When the key changes, the node unsubscribes from the old target and
    /// subscribes to the new one (deferred edge op, cycle-checked). Only the
    /// key and the current target are subscribed — inactive alternatives cost
    /// nothing and schedule nothing.
    ///
    /// A re-subscription that would close a dependency cycle is rejected and
    /// counted (`stats().cycle_rejections`); the previous target stays
    /// subscribed. Resolving to a foreign runtime's handle is ignored the
    /// same way. Resolution runs inside the purity guard: writes are rejected.
    pub fn select<K, T, F>(&self, key: &ReadSignal<K>, resolve: F) -> ReadSignal<T>
    where
        K: Clone + PartialEq + 'static,
        T: Clone + PartialEq + 'static,
        F: FnMut(&K) -> ReadSignal<T> + 'static,
    {
        self.select_with(key, resolve, PartialEq::eq)
    }

    /// [`select`](Self::select) without equality suppression.
    pub fn select_no_eq<K, T, F>(&self, key: &ReadSignal<K>, resolve: F) -> ReadSignal<T>
    where
        K: Clone + PartialEq + 'static,
        T: Clone + 'static,
        F: FnMut(&K) -> ReadSignal<T> + 'static,
    {
        self.select_with(key, resolve, |_, _| false)
    }

    fn select_with<K, T, F, E>(
        &self,
        key: &ReadSignal<K>,
        mut resolve: F,
        mut eq: E,
    ) -> ReadSignal<T>
    where
        K: Clone + PartialEq + 'static,
        T: Clone + 'static,
        F: FnMut(&K) -> ReadSignal<T> + 'static,
        E: FnMut(&T, &T) -> bool + 'static,
    {
        self.graph.ensure_scope(self.id, "Scope::select");
        if !key.same_graph(&self.graph) {
            panic!("aqua-reactive: Scope::select key belongs to a different Runtime (rejected)");
        }
        let key_cell = key.cell.clone();
        let key_node = key.node;
        let key_value = key.get();
        // Initial resolution runs before linking: failure installs nothing.
        // Guarded: resolution is derivation and must stay read-only.
        let initial_target = self.graph.guarded_eval(|| resolve(&key_value));
        if !initial_target.same_graph(&self.graph) {
            panic!("aqua-reactive: Scope::select resolved a foreign Runtime handle (rejected)");
        }
        let cell = Rc::new(RefCell::new(ValueCell {
            value: initial_target.cell.borrow().value.clone(),
            version: 0,
            disposed: false,
        }));
        let out = cell.clone();
        let mut key_seen = key_cell.borrow().version;
        let mut cur_node = initial_target.node;
        let mut cur_cell = initial_target.cell.clone();
        let mut last_target_version: Option<u64> = Some(initial_target.cell.borrow().version);
        let self_id_holder = Rc::new(RefCell::new(None::<NodeId>));
        let self_id_writer = self_id_holder.clone();
        let graph = self.graph.clone();
        let graph_check = self.graph.clone();
        let thunk = Thunk::Derive(Box::new(move || {
            if key_cell.borrow().disposed {
                return false;
            }
            let key_version = key_cell.borrow().version;
            if key_version != key_seen {
                let key_value = key_cell.borrow().value.clone();
                let next = resolve(&key_value);
                // Commit the key version only after resolve succeeded.
                key_seen = key_version;
                if next.same_graph(&graph_check) && next.node != cur_node {
                    if let Some(this) = *self_id_writer.borrow() {
                        if let Some((_, _, edge_ops, _)) = graph.upgrade_all() {
                            edge_ops.borrow_mut().push(EdgeOp::Relink {
                                node: this,
                                remove: vec![cur_node],
                                add: vec![next.node],
                            });
                        }
                    }
                    cur_node = next.node;
                    cur_cell = next.cell.clone();
                    last_target_version = None;
                }
            }
            if cur_cell.borrow().disposed {
                return false;
            }
            let tv = cur_cell.borrow().version;
            if last_target_version == Some(tv) {
                return false;
            }
            let value = cur_cell.borrow().value.clone();
            last_target_version = Some(tv);
            let mut guard = out.borrow_mut();
            if eq(&guard.value, &value) {
                return false;
            }
            guard.value = value;
            guard.version = guard.version.wrapping_add(1);
            true
        }));
        let cell_clone = cell.clone();
        let (node, slot) = self.alloc_member(
            NodeKind::Derived,
            Some(Box::new(move || {
                cell_clone.borrow_mut().disposed = true;
            })),
        );
        *self_id_holder.borrow_mut() = Some(node);
        if let Some((_, thunks, _, _)) = self.graph.upgrade_all() {
            thunks.borrow_mut().insert(node, thunk);
        }
        self.link(key_node, node, "Scope::select");
        self.link(cur_node, node, "Scope::select");
        ReadSignal {
            graph: self.graph.clone(),
            node,
            slot,
            cell,
        }
    }

    /// Coalescing batch scoped to this scope's runtime. See
    /// [`Runtime::batch`].
    pub fn batch<R>(&self, f: impl FnOnce() -> R) -> R {
        self.graph
            .runtime()
            .expect("aqua-reactive: Scope::batch after Runtime was dropped")
            .batch(f)
    }

    /// Flush queued effects. See [`Runtime::flush`].
    pub fn flush(&self) -> FlushReport {
        self.graph
            .runtime()
            .expect("aqua-reactive: Scope::flush after Runtime was dropped")
            .flush()
    }

    // -- internals ---------------------------------------------------------

    fn alloc_member(&self, kind: NodeKind, on_dispose: Option<Box<dyn Fn()>>) -> (NodeId, Slot) {
        let inner = self.graph.upgraded().expect("scope checked live above");
        let mut guard = inner.borrow_mut();
        let (id, slot) = guard.alloc_node(self.id, kind, on_dispose);
        if let Some(scope) = guard.scopes.get_mut(&self.id) {
            scope.members.push(id);
        }
        (id, slot)
    }

    fn link(&self, dep: NodeId, dependent: NodeId, what: &str) {
        let inner = self.graph.upgraded().expect("scope checked live above");
        let mut guard = inner.borrow_mut();
        if !guard.add_edge_checked(dep, dependent) {
            panic!("aqua-reactive: {what} could not link dependency {dep:?} -> {dependent:?}");
        }
    }

    fn ensure_inputs<D: DepTuple>(&self, deps: &D, what: &str) {
        let mine = self.graph.upgraded().map(|rc| Rc::as_ptr(&rc) as *const ());
        let ptrs = deps.__graph_ptrs();
        if mine.is_none_or(|m| ptrs.iter().any(|p| *p != m)) {
            panic!(
                "aqua-reactive: {what} mixes handles from different Runtimes (rejected; \
                 signals carry runtime identity)"
            );
        }
    }
}
