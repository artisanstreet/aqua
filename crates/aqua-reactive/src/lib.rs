//! `aqua-reactive`: standalone explicit-dependency reactive graph.
//!
//! No implicit automatic dependency subscription. A computation observes
//! exactly the [`ReadSignal`] handles passed to it at creation as a
//! [`DepTuple`]; reading a handle anywhere else is a snapshot and subscribes
//! to nothing. Moving code into a helper cannot change the dependency set.
//!
//! # Ownership
//!
//! A [`Runtime`] owns the graph. [`Scope`]s own signals, derived values,
//! effects, and cleanups; disposing a scope cascade-disposes its members and
//! their dependents, releases edges, drops computation closures, and marks
//! typed cells — map entries are reclaimed, not tombstoned, so create/dispose
//! churn returns live counters to baseline (see [`Runtime::stats`]).
//!
//! # Scheduling
//!
//! Writes propagate eagerly through derived values in topological order
//! (glitch-free diamonds; reads are always fresh), while effects are queued
//! and run at the update boundary: synchronously at the outermost
//! [`Scope::batch`]/[`Runtime::batch`] exit when `auto_flush` is on
//! (default), or on explicit [`Runtime::flush`] / a wake callback when the
//! host UI scheduler drives flushing.
//!
//! # Example
//!
//! ```
//! use aqua_reactive::Runtime;
//!
//! let rt = Runtime::new();
//! let scope = rt.create_scope();
//! let (count, set_count) = scope.signal(0);
//! let label = scope.derive((count.clone(),), |(n,)| format!("n={n}"));
//! assert_eq!(label.get(), "n=0");
//! set_count.set(2);
//! assert_eq!(label.get(), "n=2");
//! ```
//!
//! The full contract — dynamic rules, error rules, batching rules, and the
//! integration surface for `aqua-ui` and the `aqua` facade — lives in
//! `docs/REACTIVITY.md` at the workspace root.

mod graph;
mod resource;
mod runtime;
mod signals;
pub mod worker;

pub use graph::NodeId;
pub use resource::{
    CompleteOutcome, FetchRequest, FetchTicket, Resource, ResourceState, StaleReason,
};
pub use runtime::{FlushReport, GraphError, GraphStats, Retention, Runtime, Scope};
pub use signals::{DepTuple, EffectHandle, ReadSignal, WriteSignal};
