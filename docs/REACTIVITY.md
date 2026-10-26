# Aqua reactivity: explicit dependency graph (`aqua-reactive`)

Owner: reactive workstream. Crate: `crates/aqua-reactive` (zero dependencies).
Status: implemented, tested (26 integration tests + 1 doctest, `cargo test`;
`cargo clippy` clean). Single-threaded (`!Send + !Sync` handles) for the
initial UI executor; cross-thread handoff is a facade-level message concern,
not a handle concern.

## For UI (`aqua-ui`) and facade (`aqua`) integrators — the contract

```rust
use aqua_reactive::Runtime;

let rt = Runtime::new();
let scope = rt.create_scope();          // owner; dispose() releases everything

let (value, set_value) = scope.signal(0i32);          // (ReadSignal<T>, WriteSignal<T>)
let label = scope.derive1(&value, |v| format!("{v}")); // read-only derived
let total = scope.derive((a.clone(), b.clone()), |(a, b)| a + b); // N-tuple inputs (1–8)

let _binding: EffectHandle = scope.subscribe(&label, |text| {
    // external sync: set a UI property, log, call a native API
});
let _fx: EffectHandle = scope.effect((x.clone(), y.clone()), |(x, y)| { /* ... */ });

scope.batch(|| { set_value.set(1); set_value.set(2); }); // one coalesced effect run
rt.flush();                          // explicit update-boundary flush
rt.set_auto_flush(false);              // host-driven scheduling...
rt.set_wake_callback(|| { /* schedule rt.flush() */ }); // ...with wake hook
rt.pending();                          // queued effect count for the scheduler
rt.stats() -> GraphStats              // sources/derived/effects/edges/scopes/queue/counters
rt.retention() -> Retention           // strong-count audit; all zero after teardown
```

Key guarantees you can rely on:

- **Reads are fresh.** Writes propagate eagerly through derives in
  topological order (glitch-free diamonds) before any effect runs. There is
  no stale-read window and no `read-your-writes` caveat inside or outside
  batches.
- **Effects are the only subscription.** `subscribe(handle, f)` is the
  binding primitive for UI properties. Effects queue on change and run at the
  boundary (auto-flush at the outermost `batch` exit, or explicit
  `flush()` / wake callback when `auto_flush` is false).
- **No implicit tracking.** Reading a handle (`get`/`snapshot`) never
  subscribes. Passing more/fewer inputs to `derive`/`effect` is a compile-
  visible tuple change, not a hidden runtime behavior change.
- **Read vs write is typed.** `ReadSignal<T>` observes; `WriteSignal<T>`
  mutates. Derived values never yield write handles. A label cannot
  accidentally gain mutation authority over its inputs.
- **Inspect freely.** `stats()` never mutates the graph; identical results
  with inspection on or off.

## Ownership and disposal

`Runtime` holds the only strong references. Handles and scopes hold `Weak`
pointers only, so cycles are impossible and a dropped runtime is diagnosed
(`GraphError::RuntimeDropped` / `try_get() == None`), never UB.

- `Scope::dispose()` cascade-disposes members **and their dependents**,
  detaches edges, drops computation closures, marks typed cells, purges
  queued effects, runs cleanups LIFO, and recursively disposes children.
- Map entries are **removed, not tombstoned**: steady create/dispose churn
  (e.g. keyed list rows) returns live counters to baseline — covered by the
  `create_dispose_churn_returns_to_baseline` test (500 iterations).
- Node ids are monotonic and never reused, so a stale handle can never alias
  a newer node; disposal is diagnosed explicitly.
- Disposing a source kills its declared dependents (even in other scopes):
  shared long-lived sources must live in a scope that outlives borrowers.
  Disposing a leaf scope only detaches it from its sources.

## Scheduling

- Source write → eager topological recompute of dependent derives (with
  equality suppression) → dependent effects queued (deduped) → flush.
- `batch` coalesces: N writes inside one batch run dependent effects once
  with the latest values. Batches nest; only the outermost exit flushes.
- A batch is **not** a transaction: writes commit immediately, and a panic
  does not roll back — it truncates effects queued by the failed batch and
  restores depth, so outer batches and later updates stay coherent (tested).
- Effects may write signals (forward sync); those writes queue further
  effects in the same flush, bounded by `set_flush_limit` (default 1024).
  Exceeding it stops the flush with `truncated: true` and counts
  `flush_truncations` — a feedback loop becomes a diagnostic, not a hang.
- Reentrant `flush()` from inside an effect is a no-op (`reentered: true`);
  the outer flush owns the queue. No nested synchronous flushing.
- `derive_depth` purity guard: signal writes while a derived computation
  (or `select` resolution) evaluates are rejected with
  `GraphError::DerivedWrite`. Derived values are pure and read-only; move
  writes into effects, handlers, or setup code. This also covers initial
  computations and re-resolution, not just triggered recomputes.

## Dynamic dependencies (explicit operators only)

Dependency sets are fixed at creation **except** through `select`:

```rust
let view = scope.select(&key, move |k| if *k == 0 { tab_a.clone() } else { tab_b.clone() });
```

- Subscribes to `key` plus the *current* target only. Inactive alternatives
  cost nothing and schedule nothing.
- Key change → deferred edge relink (applied outside user code, cycle-checked).
- A relink that would close a cycle is rejected and counted
  (`stats().cycle_rejections`); the previous target stays subscribed.
- Resolving to a foreign runtime's handle is likewise rejected (runtime
  identity is carried by handles; mixing panics at creation, is ignored at
  re-resolution).
- Toggle branches repeatedly: inactive sources stop scheduling work (tested).

There is no `untrack`/snapshot-context operator because there is no implicit
context to escape: every read outside a declared input list is already a
snapshot (`ReadSignal::get` / `snapshot`).

## Batching, events, and equality

- Multiple signal writes coalesce to one final-value effect run. If every
  occurrence must be delivered (keystrokes, audit records), model them as an
  explicit event queue signal, not a coalescible value — lossless events are
  a separate facility from coalesced state observation.
- `WriteSignal::set` always notifies; `set_if_changed` (requires
  `PartialEq`) suppresses equal writes and reports whether it committed.
  NaN never equals itself (always notifies); large-collection comparison cost
  is the caller's choice.
- `derive` requires `T: PartialEq` and suppresses downstream notification on
  equality; `derive_no_eq` always notifies; `derive_with` takes a custom
  comparator. Choose per node; do not assume one policy wins.
- `try_derive` stores `Result<T, E>` as an observable value with
  retry-on-input-change: `Err` propagates as data, and changing any input
  recomputes and may recover to `Ok` (tested both directions).

## Error rules

- Creation on a dead scope / foreign-runtime inputs: panic with diagnostic
  (programmer error, fail fast).
- Fallible reads/writes (`try_get`, `try_set`, `try_update`): `Ok`/`Err`
  (`Disposed`, `RuntimeDropped`, `DerivedWrite`) for legitimately racy paths
  (worker completion, teardown).
- Panic inside a derive: the node's previous value, versions, and edges are
  untouched (versions commit only after user code succeeds); the panic
  continues and the next trigger retries cleanly.
- Panic inside an effect: the remaining queue is preserved; the failed effect
  retries on the next flush. The flushing flag resets via a guard.
- Panic inside a batch: effects queued by the failed batch are truncated,
  depth restored; committed writes persist.

## Performance notes (correctness first, budget visible)

- Rank caching: each node caches its longest-path level; edge mutations set
  `ranks_dirty`, and propagation re-sorts only the dirty subgraph by
  `(rank, id)` instead of running Kahn per write. Scratch buffers are reused.
- The effect queue is deduplicated (one entry per effect); per-node version
  checks make re-runs and re-propagations no-ops.
- Follow-up (not yet done): arena/dense storage for hot records, dirty
  worklists avoiding full subgraph collection on deep chains. Contributions
  welcome provided the tests above stay green.

## Async resources (`Scope::resource`)

`resource(deps, params_of)` derives fetch *parameters* from declared inputs;
the async work runs on a host executor, never in the graph. State is an
observable `ResourceState::{Loading, Ready, Failed, Idle}` with generations:

- Input change / `reload()` → generation bump, `Loading`, one coalesced
  `FetchRequest { ticket, params }` in the outbox. Drain with
  `take_requests()` after each flush; `set_notify` wakes the host loop
  without polling.
- Host runs the future anywhere, then calls `complete(ticket, result)` on the
  owner thread. Only the current generation against a live owner commits;
  `Superseded` / `Cancelled` / `Duplicate` / `Disposed` outcomes are rejected
  without touching state (no stale-success flash) and counted in
  `stats().stale_completions`.
- `cancel()` invalidates in-flight work and resets to `Idle`; scope disposal
  rejects all later completions as `Disposed`.
- `crate::worker::Worker` is the bundled blocking-thread helper:
  `dispatch(ticket, params, work)` runs `work` off-thread, `take_completed()`
  drains type-erased results for owner-thread commitment, and the wake hook
  fires on arrival. See `examples/resource_worker.rs` (wiring) and
  `tests/resource.rs` (9 race tests: reload, cancel, dispose, duplicate,
  coalescing, failure/retry, worker end-to-end).

## Measured scaling (`cargo run --release --example scaling`)

Steady-state, counting allocator, warmed rank cache (Linux x86-64, release):

| case | ns/update | bytes/update | allocs/update |
|---|---|---|---|
| wide/10 (1 source, 10 derives) | ~510 | 0 | 0 |
| wide/1000 | ~48,000 (~48 ns/derive) | 0 | 0 |
| wide/10000 | ~579,000 (~58 ns/derive) | 0 | 0 |
| chain/1000 (depth 1000) | ~50,000 (~50 ns/link) | 0 | 0 |

Cost is linear in affected nodes with zero per-update allocation (dense
slots, stamped dirty filter over a cached rank order, reused buffers). No
further dense-storage work warranted at this scale; revisit with profiler
evidence if UI-sized graphs disagree.

## Deferred (out of scope for this crate)
- Cross-thread handles: facade message handoff only.
- Keyed collections, lenses, field-level granularity: UI-layer facilities
  over these primitives; `Signal<Vec<T>>` is collection-level state until
  then.
