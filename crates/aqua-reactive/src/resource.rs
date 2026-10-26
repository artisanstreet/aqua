//! Async resource lifecycle with explicit inputs and generation tickets.
//!
//! A `Resource` derives its *request parameters* from declared reactive
//! inputs, but the async work itself runs on a host executor — never in the
//! graph. Completion carries a [`FetchTicket`]; only the ticket matching the
//! current generation against a live owner commits. Everything else is stale
//! and counted, never flashed.
//!
//! The task executor is deliberately separate: the graph emits
//! [`FetchRequest`]s into the resource outbox, the host drains them
//! ([`Resource::take_requests`]), runs the future on its own runtime, and
//! calls [`Resource::complete`] back on the owner thread. [`crate::worker`]
//! provides a small blocking-thread helper plus a wake hook so the host
//! event loop wakes on arrival instead of polling.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::graph::{NodeId, Slot};
use crate::runtime::GraphRef;
use crate::signals::{ReadSignal, WriteSignal};

/// Outcome of attempting to commit a host completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompleteOutcome {
    /// The ticket matched the current generation of a live owner; state committed.
    Committed,
    /// The completion was rejected without touching state (counted in stats).
    Stale(StaleReason),
}

/// Why a completion was rejected as stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaleReason {
    /// A newer generation was requested (reload or input change) first.
    Superseded,
    /// The generation was cancelled via [`Resource::cancel`].
    Cancelled,
    /// This exact ticket already committed (host retried delivery).
    Duplicate,
    /// The owner node is gone (scope disposed) or the ticket names a
    /// different resource.
    Disposed,
}

/// Observable lifecycle state of a resource. The generation lets consumers
/// distinguish "loading the thing I asked for" from obsolete transitions;
/// every transition commits atomically, so observers never see a stale
/// success flash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceState<T, E> {
    /// A fetch is in flight for `generation`. Superseded by any larger generation.
    Loading { generation: u64 },
    /// The current generation completed successfully.
    Ready { generation: u64, value: T },
    /// The current generation failed.
    Failed { generation: u64, error: E },
    /// No fetch in flight and no usable value (initial pre-first-run is
    /// `Loading { generation: 0 }`; `cancel` of an in-flight fetch lands here).
    Idle,
}

/// Typed ticket binding a completion to one generation of one resource.
/// `Send + Sync`: safe to move across threads to the executor and back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FetchTicket {
    pub node: NodeId,
    pub slot: Slot,
    pub generation: u64,
}

/// One unit of host work: run the fetch for `params`, then
/// [`Resource::complete`] the `ticket` back on the owner thread.
pub struct FetchRequest<P> {
    pub ticket: FetchTicket,
    pub params: P,
}

pub(crate) struct ResourceShared<P> {
    pub(crate) generation: u64,
    pub(crate) committed: Option<u64>,
    pub(crate) last_cancelled: Option<u64>,
    pub(crate) in_flight: bool,
    pub(crate) last_params: Option<P>,
    pub(crate) outbox: VecDeque<FetchRequest<P>>,
    pub(crate) notify: Option<Box<dyn Fn()>>,
}

impl<P> ResourceShared<P> {
    pub(crate) fn new() -> Self {
        Self {
            generation: 0,
            committed: None,
            last_cancelled: None,
            in_flight: false,
            last_params: None,
            outbox: VecDeque::new(),
            notify: None,
        }
    }
}

/// Async resource with explicit inputs. Owns a state signal (driven only via
/// `request`/`complete`/`reload`/`cancel`), an internal input-tracking
/// effect, and a typed request outbox for the host executor.
///
/// Dropping the `Resource` handle does not cancel anything (the owner scope
/// owns the nodes); disposing the scope rejects all later completions as
/// [`StaleReason::Disposed`].
#[derive(Clone)]
pub struct Resource<T, E, P> {
    graph: GraphRef,
    node: NodeId,
    slot: Slot,
    state_r: ReadSignal<ResourceState<T, E>>,
    state_w: WriteSignal<ResourceState<T, E>>,
    shared: Rc<RefCell<ResourceShared<P>>>,
}

impl<T, E, P> Resource<T, E, P>
where
    T: Clone + PartialEq + 'static,
    E: Clone + PartialEq + 'static,
    P: Clone + 'static,
{
    pub(crate) fn new(
        graph: GraphRef,
        node: NodeId,
        slot: Slot,
        state_r: ReadSignal<ResourceState<T, E>>,
        state_w: WriteSignal<ResourceState<T, E>>,
        shared: Rc<RefCell<ResourceShared<P>>>,
    ) -> Self {
        Self {
            graph,
            node,
            slot,
            state_r,
            state_w,
            shared,
        }
    }

    /// Live read handle to the resource state. Bind UI to this.
    pub fn state(&self) -> ReadSignal<ResourceState<T, E>> {
        self.state_r.clone()
    }

    /// Current generation (advanced by every request and cancel).
    pub fn generation(&self) -> u64 {
        self.shared.borrow().generation
    }

    /// True when a fetch for the current generation has been requested and
    /// neither completed nor cancelled.
    pub fn is_in_flight(&self) -> bool {
        self.shared.borrow().in_flight
    }

    /// Drain queued fetch requests for the host executor. Deterministic:
    /// call after each flush (or on the notify hook).
    pub fn take_requests(&self) -> Vec<FetchRequest<P>> {
        self.shared.borrow_mut().outbox.drain(..).collect()
    }

    /// Host wake hook, invoked (on the owner thread) whenever a request is
    /// enqueued. Lets the host event loop wake without polling the outbox.
    /// Replaces any previous hook.
    pub fn set_notify(&self, f: impl Fn() + 'static) {
        self.shared.borrow_mut().notify = Some(Box::new(f));
    }

    /// Enqueue a fetch for `params`: advance the generation, publish
    /// `Loading`, queue the ticket, and invoke the notify hook.
    /// `crate::Scope::resource`'s input-tracking effect calls this.
    pub(crate) fn track_request(&self, params: P) {
        let notify = {
            let mut shared = self.shared.borrow_mut();
            if self
                .graph
                .upgraded()
                .is_none_or(|inner| !inner.borrow().handle_live(self.node, self.slot))
            {
                return;
            }
            shared.generation += 1;
            shared.in_flight = true;
            shared.last_params = Some(params.clone());
            let ticket = FetchTicket {
                node: self.node,
                slot: self.slot,
                generation: shared.generation,
            };
            shared.outbox.push_back(FetchRequest { ticket, params });
            shared.last_cancelled = None;
            shared.notify.is_some()
        };
        let _ = self.state_w.try_set(ResourceState::Loading {
            generation: self.shared.borrow().generation,
        });
        if notify {
            let cb = self.shared.borrow_mut().notify.take();
            if let Some(cb) = cb {
                cb();
                self.shared.borrow_mut().notify = Some(cb);
            }
        }
    }

    /// Refetch with the last delivered params (retry after failure, refresh).
    /// Returns false when there is nothing to reload or the owner is gone.
    pub fn reload(&self) -> bool {
        let params = self.shared.borrow().last_params.clone();
        match params {
            Some(p) => {
                self.track_request(p);
                true
            }
            None => false,
        }
    }

    /// Invalidate the in-flight fetch, if any, and reset to `Idle`.
    /// In-flight completions afterwards are stale (`Cancelled`). Returns true
    /// when a fetch was actually cancelled.
    pub fn cancel(&self) -> bool {
        {
            let mut shared = self.shared.borrow_mut();
            if !shared.in_flight {
                return false;
            }
            shared.last_cancelled = Some(shared.generation);
            shared.generation += 1;
            shared.in_flight = false;
            shared.outbox.clear();
        }
        // State write outside the shared borrow; a dead owner just fails.
        let _ = self.state_w.try_set(ResourceState::Idle);
        true
    }

    /// Attempt to commit a host completion. Only the ticket matching the
    /// current generation against a live owner commits; everything else is
    /// stale and counted in `stats().stale_completions`. Never flashes.
    pub fn complete(&self, ticket: &FetchTicket, result: Result<T, E>) -> CompleteOutcome {
        if ticket.node != self.node {
            self.count_stale();
            return CompleteOutcome::Stale(StaleReason::Disposed);
        }
        let live = self
            .graph
            .upgraded()
            .is_some_and(|inner| inner.borrow().handle_live(self.node, self.slot));
        if !live {
            self.count_stale();
            return CompleteOutcome::Stale(StaleReason::Disposed);
        }
        let decision = {
            let shared = self.shared.borrow();
            if Some(ticket.generation) == shared.committed {
                Err(StaleReason::Duplicate)
            } else if ticket.generation != shared.generation {
                if Some(ticket.generation) == shared.last_cancelled {
                    Err(StaleReason::Cancelled)
                } else {
                    Err(StaleReason::Superseded)
                }
            } else {
                Ok(())
            }
        };
        match decision {
            Err(reason) => {
                self.count_stale();
                CompleteOutcome::Stale(reason)
            }
            Ok(()) => {
                let state = match result {
                    Ok(value) => ResourceState::Ready {
                        generation: ticket.generation,
                        value,
                    },
                    Err(error) => ResourceState::Failed {
                        generation: ticket.generation,
                        error,
                    },
                };
                {
                    let mut shared = self.shared.borrow_mut();
                    shared.committed = Some(ticket.generation);
                    shared.in_flight = false;
                }
                // Owner was verified live above; a concurrent dispose just
                // fails the write, which is itself a coherent outcome.
                let _ = self.state_w.try_set(state);
                CompleteOutcome::Committed
            }
        }
    }

    fn count_stale(&self) {
        if let Some(inner) = self.graph.upgraded() {
            inner.borrow_mut().stale_completions += 1;
        }
    }
}
