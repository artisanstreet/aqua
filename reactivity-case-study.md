**Svelte and Solid reactivity: failure modes and lessons for a Rust GUI framework**

Research date: 2026-10-24. This is a catalog of 60 sourced failure modes, semantic traps, and implementation incidents, followed by proposed Rust-specific requirements. It is not an exhaustive audit of every issue or release. Related symptoms are sometimes separate entries because they require different runtime invariants or tests.

Evidence labels: **Documented** means intended behavior or an explicit limitation; **Historical** means an older model or version; **Fixed** means a merged upstream fix or a published changelog entry; **Reported** means an upstream report or proposed fix, not independently reproduced here. A merged fix does not establish the first release containing it. Proposed requirements and tests below are our analysis, not claims made by the upstream projects.

Scope: Svelte legacy reactivity, Svelte 5 runes and experimental async behavior, and Solid's documented 1.x APIs. Solid 2 design discussions are identified separately. Browser hydration details are included only where they illustrate a transferable consistency or ownership problem. These entries were researched through official documentation, repository issues, pull requests, changelogs, and maintainer discussions; no framework reproduction suite was run.

**Legacy Svelte: lessons that should not be attributed to modern signals**

| ID | Finding and evidence | Requirement / regression scenario for us |
|---|---|---|
| 01 | **Historical:** `$:` dependencies were inferred from visible references; moving a read into a helper could hide it. [Legacy reactive statements](https://svelte.dev/docs/svelte/legacy-reactive-assignments#Understanding-dependencies) | Runtime tracking must see reads through ordinary Rust calls. Extract an expression into a helper and verify identical subscriptions. |
| 02 | **Historical:** mutation methods such as array `push` did not themselves trigger legacy assignment-based reactivity. [Legacy declarations](https://svelte.dev/docs/svelte/legacy-let) | Give mutation an explicit observable boundary. Test equivalent collection edits through every supported API. |
| 03 | **Historical report:** once-per-tick reactive blocks could miss intermediate changes and fail to converge for state-writing reactions. [Issue #6732](https://github.com/sveltejs/svelte/issues/6732) | Separate coalesced state observation from lossless events. Test multiple writes and feedback within one turn; do not suppress necessary propagation to hide loops. |
| 04 | **Historical:** implicit top-level reactivity constrained refactoring into reusable functions; Svelte 5 made state explicit. [Migration rationale](https://svelte.dev/docs/svelte/v5-migration-guide#Reactivity-syntax-changes-let-%E2%86%92-%24state) | Signals must work in ordinary Rust modules and model code. Moving state creation outside a UI function should not change semantics. |

**Tracking: what a read actually means**

| ID | Finding and evidence | Requirement / regression scenario for us |
|---|---|---|
| 05 | **Documented, Solid:** destructuring or eagerly copying reactive props can lose live access. [Props](https://docs.solidjs.com/concepts/components/props) | Distinguish `T` snapshots from `ReadSignal<T>` handles in types. Test prop replacement after a child mounts. |
| 06 | **Documented, Svelte:** passing a state value into a function passes its current value, not a live variable reference. [Passing state into functions](https://svelte.dev/docs/svelte/$state#Passing-state-into-functions) | APIs that expect live input accept handles or accessors explicitly. Test snapshot and live-input paths separately. |
| 07 | **Documented:** conditional computations track the branch actually read on their last execution. [Solid fine-grained reactivity](https://docs.solidjs.com/advanced-concepts/fine-grained-reactivity) | Replace dependency sets after reruns. Toggle branches repeatedly and verify inactive sources stop scheduling work. |
| 08 | **Documented, Svelte:** synchronous reads inside called helpers also become effect dependencies. [Effect dependencies](https://svelte.dev/docs/svelte/$effect#Understanding-dependencies) | Incidental logging, validation, and callbacks can over-subscribe. Provide `untrack` and show the call site of tracked reads in the inspector. |
| 09 | **Documented, Solid:** ordinary async callbacks run after the synchronous tracking context has ended. [Fine-grained reactivity](https://docs.solidjs.com/advanced-concepts/fine-grained-reactivity#synchronous-nature-of-effect-tracking) | Define tracking per synchronous computation. Test a deferred read, and provide explicit resource inputs for async work. |
| 10 | **Documented, Solid:** restoring an owner does not restore dependency tracking, and neither survives an ordinary `await`. [runWithOwner](https://docs.solidjs.com/reference/reactive-utilities/run-with-owner) | Keep ownership context and tracking context separate. Test each independently rather than representing both with one global pointer. |
| 11 | **Documented, Solid:** event handler bindings are not generally reactive; a wrapper can read the current callback prop on invocation. [Event handlers](https://docs.solidjs.com/concepts/components/event-handlers) | Decide between captured callbacks and replaceable callback slots. Replace a handler prop and assert which function runs. |
| 12 | **Documented, Solid:** ordinary object merging can lose reactive property access; `mergeProps` and `splitProps` preserve it. [Props](https://docs.solidjs.com/concepts/components/props) | Passing or composing component properties must preserve live handles. Test defaults, forwarding, and overriding properties. |

**Mutation, granularity, and identity**

| ID | Finding and evidence | Requirement / regression scenario for us |
|---|---|---|
| 13 | **Documented, Solid:** stores track property reads; a signal represents one value. [createStore](https://docs.solidjs.com/reference/store-utilities/create-store) | `Signal<Vec<T>>` does not imply field-level subscriptions. Test whether changing one row invalidates one field, one row, or the full collection. |
| 14 | **Documented, Solid:** signal equality normally uses `===`; returning the same mutated object can suppress notification. [createSignal](https://docs.solidjs.com/reference/basic-reactivity/create-signal) | Define `set`, `update`, equality suppression, and explicit notification separately. Test in-place mutation and same-value writes. |
| 15 | **Documented, Svelte:** deep proxying handles arrays and simple objects, with boundaries at classes and other object kinds. [Deep state](https://svelte.dev/docs/svelte/$state#Deep-state) | Avoid a promise of universal automatic deep tracking. Specify which model fields and container operations are observable. |
| 16 | **Documented, Svelte:** derived results are not automatically converted into deeply reactive objects. [Deriveds and reactivity](https://svelte.dev/docs/svelte/$derived#Deriveds-and-reactivity) | A computed collection needs explicit item handles or projections if nested editing is supported. Test edits through a derived selection. |
| 17 | **Documented, Svelte:** reactive Map and Set wrappers do not automatically make their contained values deeply reactive. [Reactive built-ins](https://svelte.dev/docs/svelte/svelte-reactivity) | Separate membership, lookup, iteration, length, and item-value dependencies. Test each kind of observer against insert, update, and remove. |
| 18 | **Documented, Svelte:** proxy identity can differ from the original object's identity. [Proxy equality warning](https://svelte.dev/docs/svelte/runtime-warnings#state_proxy_equality_mismatch) | Use explicit stable IDs for model and UI identity. Do not derive keys from wrapper allocation or storage addresses. |
| 19 | **Documented, Svelte:** an assignment expression can return the raw assigned value while the property stores a proxy; chained mutation can target the wrong object. [Stale assignment warning](https://svelte.dev/docs/svelte/runtime-warnings#assignment_value_stale) | Mutation APIs should return the actual observable target or no target at all. Test chained insertion-and-edit operations. |
| 20 | **Documented, Svelte:** reassigned rune state cannot simply be exported across modules because compilation is per file. [Passing state across modules](https://svelte.dev/docs/svelte/$state#Passing-state-across-modules) | Public Rust state APIs must work without rewriting consumers. Test state shared through a separately compiled crate. |
| 21 | **Documented, Svelte:** mutating another component's reactive props can work while violating ownership expectations; writable bindings are explicit. [Updating props](https://svelte.dev/docs/svelte/$props#Updating-props) | Use separate read and write capabilities. A child should only edit state when passed a writable handle or command. |
| 22 | **Documented, Svelte:** replacing a state object previously placed in context can break the connection to consumers. [Context with state](https://svelte.dev/docs/svelte/context#Using-context-with-state) | Context should normally contain a stable signal/store handle. Test replacing the model without remounting descendants. |

**Scheduling and computed state**

| ID | Finding and evidence | Requirement / regression scenario for us |
|---|---|---|
| 23 | **Documented, Solid:** memos should be pure, allowing consistent dependency ordering and downstream equality suppression. [createMemo](https://docs.solidjs.com/reference/basic-reactivity/create-memo) | Test a diamond graph: both computed branches must be current before an observer sees their combination. Reject signal writes during derivation. |
| 24 | **Documented, Svelte:** effects that read and write their dependencies can loop; recursive derived reads are also rejected. [Runtime errors](https://svelte.dev/docs/svelte/runtime-errors#effect_update_depth_exceeded) | Detect cycles with useful paths. Distinguish excessive legitimate work from repeated traversal of the same cycle. |
| 25 | **Documented, Solid:** batching defers observers, but an explicit read inside a batch can refresh a stale memo. [batch](https://docs.solidjs.com/reference/reactive-utilities/batch) | A batch is not automatically snapshot isolation. Specify read-your-writes behavior; test reads between two writes. |
| 26 | **Documented, Solid:** an async batch only covers work before the first suspension. [batch](https://docs.solidjs.com/reference/reactive-utilities/batch) | Do not keep UI transactions open across arbitrary futures. Test writes before and after suspension. |
| 27 | **Documented:** effect timing differs: Svelte schedules ordinary effects after UI updates in a microtask; Solid distinguishes ordinary and render effects. [Svelte lifecycle](https://svelte.dev/docs/svelte/$effect#Understanding-lifecycle), [Solid effects](https://docs.solidjs.com/reference/basic-reactivity/create-effect) | Define model propagation, UI commit, layout, and presentation phases. Test what geometry and state an effect can read in each phase. |
| 28 | **Design tradeoff:** Svelte's synchronous deriveds are lazy; Solid's maintainer revisited lazy defaults during Solid 2 exploration because async and boundaries favor eager work. [Svelte propagation](https://svelte.dev/docs/svelte/$derived#Update-propagation), [Solid discussion #2416](https://github.com/solidjs/solid/discussions/2416) | Benchmark unused expensive computations and resource startup separately. Do not claim one universal eager/lazy policy wins. |

**Ownership, cleanup, and graph integrity**

| ID | Finding and evidence | Requirement / regression scenario for us |
|---|---|---|
| 29 | **Documented:** effects need an owner or an explicit root with disposal responsibility. [Solid createRoot](https://docs.solidjs.com/reference/reactive-utilities/create-root), [Svelte orphan effects](https://svelte.dev/docs/svelte/runtime-errors#effect_orphan) | Require an owner for active computations. Test repeated creation and disposal outside a window. |
| 30 | **Documented, Solid:** cleanup runs on scope refresh as well as disposal. [onCleanup](https://docs.solidjs.com/reference/lifecycle/on-cleanup) | Distinguish per-run cleanup from component destruction. Replace a dependency repeatedly and verify one active external subscription. |
| 31 | **Fixed, Svelte:** deriveds read only in untracked initialization could remain attached after destruction. Fix merged 2026-07-10. [PR #18517](https://github.com/sveltejs/svelte/pull/18517) | Reading without subscribing must not accidentally create an immortal subscription chain. Test initialization-only derived chains against a long-lived source. |
| 32 | **Fixed, Svelte:** an exception after connecting a fresh derived could leave it half-linked and retained. Fix merged 2026-08-25. [PR #18703](https://github.com/sveltejs/svelte/pull/18703) | Graph changes must remain coherent on failed computation. Inject an error after each read and check disposal removes all edges. |
| 33 | **Fixed, Svelte:** the same failure path could lose a throwing derived's dependencies and leave its error cached indefinitely. [PR #18703](https://github.com/sveltejs/svelte/pull/18703) | Define error caching and retry invalidation together. Change the input that caused an error and verify recovery. |
| 34 | **Reported/proposed, Svelte:** historical reads during cleanup could rewire live dependencies back to old nested state; PR remained open when checked. [PR #18967](https://github.com/sveltejs/svelte/pull/18967) | Historical cleanup reads need isolation from the live graph. Replace nested state, run cleanup, then mutate the replacement. |
| 35 | **Fixed, Svelte:** synchronous inspection could run while the graph was inconsistent. Fix merged 2024-09-10. [PR #13153](https://github.com/sveltejs/svelte/pull/13153) | Debugging must observe a coherent graph without changing update results. Run the same graph with inspection on and off. |
| 36 | **Reported, Solid 1.9.17:** retained row-disposer closures could keep removed list items reachable. Issue remained open when checked. [Issue #4095](https://github.com/solidjs/solid/issues/4095) | Capture only per-row data. Use drop counters to verify removing an item frees its model even while adjacent rows survive. |

**Asynchronous reactivity**

| ID | Finding and evidence | Requirement / regression scenario for us |
|---|---|---|
| 37 | **Documented maintainer discussion, Solid:** overlapping async effects can complete out of order, including external writes. [Issue #670](https://github.com/solidjs/solid/issues/670) | Resources need generation checks. Test slow request A finishing after fast request B; obsolete A must not replace B. |
| 38 | **Documented discussion, Solid:** discarding stale results and cancelling underlying work are different mechanisms; autosave also raises debounce versus serialization choices. [Issue #670](https://github.com/solidjs/solid/issues/670) | Define read-resource cancellation separately from write-command ordering. Test an already-sent save that cannot be cancelled. |
| 39 | **Documented, Svelte:** async deriveds need ownership because their eager work uses effects. [Async derived orphan](https://svelte.dev/docs/svelte/runtime-errors#async_derived_orphan) | Give resources an owner, cancellation policy, and disposal behavior. Test unmount during pending work. |
| 40 | **Documented, Svelte:** compiler-visible `await` expressions can preserve tracking that an extracted async helper loses. [Await reactivity loss](https://svelte.dev/docs/svelte/runtime-warnings#await_reactivity_loss) | Prefer explicit synchronous resource inputs for the initial design. Moving an async operation into a helper should not silently change dependencies. |
| 41 | **Documented, Svelte experimental async:** synchronized expressions can delay related visible state until their async result is ready. [Synchronized updates](https://svelte.dev/docs/svelte/await-expressions#Synchronized-updates) | Separate current input from displayed results and request parameters. Test typing while slow search results remain visible. |
| 42 | **Documented, Svelte:** async derived usage can introduce avoidable waterfalls. [Waterfall warning](https://svelte.dev/docs/svelte/runtime-warnings#await_waterfall) | Start independent requests independently. Verify two unrelated resources overlap and genuinely dependent resources remain ordered. |
| 43 | **Maintainer assessment, Solid 2 discussion:** explicit loading/error fields could compete with boundary-based behavior and surprise callers reading resource values elsewhere. [Discussion #2657](https://github.com/solidjs/solid/discussions/2657) | Choose one primary pending/error model, and make alternative views consistent with it. Test the same resource consumed inside and outside a boundary. |

**Structural updates and identity**

| ID | Finding and evidence | Requirement / regression scenario for us |
|---|---|---|
| 44 | **Documented, Solid:** `For` follows item identity; `Index` follows positions. [For](https://docs.solidjs.com/reference/components/for), [Index](https://docs.solidjs.com/reference/components/index-component) | Make identity explicit. Reorder editable rows and verify text, selection, focus, and local state follow the intended item. |
| 45 | **Documented, Svelte:** duplicate keys and non-idempotent key expressions are invalid. [Key errors](https://svelte.dev/docs/svelte/runtime-errors#each_key_duplicate) | Diagnose duplicates before mutating the live tree. Test collisions and freshly allocated keys without corrupting existing children. |
| 46 | **Documented, Solid:** non-keyed `Show` preserves a child block across truthy replacements; keyed mode recreates it. [Show](https://docs.solidjs.com/reference/components/show) | Define whether replacing selected data preserves or resets component-local state. Test A-to-B replacement while the branch remains visible. |
| 47 | **Documented, Solid:** a narrowed conditional accessor is only valid while its condition remains true. [Show](https://docs.solidjs.com/reference/components/show) | A callback or future can outlive its branch. Test delayed access after removal; expose fallible or cancelled access rather than assuming validity. |
| 48 | **Documented, Solid:** resolving children through `children` memoizes the result instead of recreating child structure on repeated reads. [children](https://docs.solidjs.com/reference/component-apis/children) | Specify whether children are instances, factories, or accessors. Test inspecting children without mounting a second copy. |
| 49 | **Historical restriction, Svelte:** assigning or binding to an each-block variable proved unpredictable with derived collections and is forbidden in runes mode. [Compiler error](https://svelte.dev/docs/svelte/compiler-errors#each_item_invalid_assignment) | Writable list bindings need stable model locations or IDs, not transient iterator values. Test filtered and sorted editable lists. |

**Additional implementation incidents worth preserving as regression cases**

| ID | Finding and evidence | Requirement / regression scenario for us |
|---|---|---|
| 50 | **Fixed, Svelte:** speculative fork evaluation updated derived values/versions and could suppress later committed updates. Fix merged 2025-11-19. [PR #17163](https://github.com/sveltejs/svelte/pull/17163) | Speculative values and versions must not contaminate committed caches. Fork, read, discard, then perform a real write. |
| 51 | **Fixed, Svelte:** failed or finished batches could remain linked and participate in later work. Fix merged 2026-05-21. [PR #18264](https://github.com/sveltejs/svelte/pull/18264) | Every terminal batch state must release scheduling links. Fail a batch, then verify unrelated later updates succeed. |
| 52 | **Fixed, Svelte:** rejected promises needed forwarding during batch merge/commit to avoid missed error handling. Fix merged 2026-05-21. [PR #18266](https://github.com/sveltejs/svelte/pull/18266) | Merging work must preserve success, failure, cancellation, and obsolescence. Test every terminal state and resolution order. |
| 53 | **Fixed, Solid 1.9.15:** selectors could remain stale after transitions. [Package changelog](https://github.com/solidjs/solid/blob/main/packages/solid/CHANGELOG.md) | Selection consumers need the same version rules as ordinary computations. Test repeated selection transitions. |
| 54 | **Fixed, Solid 1.9.15:** external-source computations could lose ordinary subscriptions after transition completion. [Package changelog](https://github.com/solidjs/solid/blob/main/packages/solid/CHANGELOG.md) | Test external subscriptions through enter, commit, cancel, and subsequent ordinary updates. |
| 55 | **Fixed, Solid 1.9.14:** module-cached lazy resources retained disposed boundary trees. [Package changelog](https://github.com/solidjs/solid/blob/main/packages/solid/CHANGELOG.md) | Keep reusable asset/module caches separate from instance-owned resources and UI scopes. Test nested lazy views across repeated mounts. |
| 56 | **Reported, Solid:** lazy-component remounts flashed empty/fallback content following cleanup changes; issue remained open when checked. [Issue #4118](https://github.com/solidjs/solid/issues/4118) | Cleanup must not invalidate other instances' valid shared caches. Test two consumers and an immediate remount after one is destroyed. |
| 57 | **Documented, Solid:** render/update error boundaries do not catch arbitrary event-handler or separately scheduled callback errors. [ErrorBoundary](https://docs.solidjs.com/reference/components/error-boundary) | Define error routing for UI expressions, commands, tasks, and platform callbacks independently. Test every entry point. |
| 58 | **Fixed, Solid 1.9.7:** resource handling needed to catch synchronous fetcher errors. [Package changelog](https://github.com/solidjs/solid/blob/main/packages/solid/CHANGELOG.md) | Immediate failure and later async failure need coherent result/error state and cleanup. Test both. |
| 59 | **Fixed, Solid 1.9.16:** nested server boundaries could complete from discarded renders, producing stale output or hanging completion. [Package changelog](https://github.com/solidjs/solid/blob/main/packages/solid/CHANGELOG.md) | Native analogue: an obsolete scene/resource generation must not commit or keep completion pending. Test discard followed by immediate successful replacement. |
| 60 | **Historical, Solid:** multiple runtime copies could break reactivity; version detection was added. [Historical changelog, 0.19](https://github.com/solidjs/solid/blob/main/CHANGELOG.md) | Signal handles need runtime identity. Test cross-window, cross-runtime, and separately loaded extension boundaries; reject unsupported mixing explicitly. |

**Two further scheduler reports that deserve caution**

Svelte [issue #18546](https://github.com/sveltejs/svelte/issues/18546), open when checked, reports nested synchronous flushing leaving dirty branches without queued work. Proposed [PR #18957](https://github.com/sveltejs/svelte/pull/18957), also open, describes commit callbacks accidentally subscribing the effect that triggered a flush. These are reported mechanisms, not independently confirmed results here.

Our derived requirements: prohibit reentrant flushing initially; separate internal commit work from user tracking contexts; verify that every dirty branch has a scheduled path to execution. A null check at the crash site would not establish that the dependency graph is correct.

**Important conclusions for our Rust design**

These are proposed requirements inferred from the evidence, plus Rust-specific engineering concerns. They are not additional upstream bug claims.

1. **Separate three structures.** Dependency edges determine invalidation. Ownership edges determine disposal. UI nodes determine visual identity. They overlap but cannot substitute for one another.
2. **Use runtime dependency tracking through Rust functions.** The macro establishes computation boundaries; signal reads register dependencies. A proc macro cannot inspect arbitrary separately compiled function bodies.
3. **Give source state and derived values different capabilities.** Start with read-only derived values. Treat optimistic local overrides as an explicit facility rather than making every computed value writable.
4. **Make writable bindings capabilities.** `ReadSignal<T>` conveys observation; `WriteSignal<T>` or a model lens conveys mutation authority. A computed iterator item is not automatically writable storage.
5. **Specify observable granularity.** A signal containing a struct or vector is one dependency unless projections, field signals, or store instrumentation say otherwise. `update` alone cannot infer which arbitrary Rust fields changed.
6. **Avoid whole-graph locks or mutable borrows across user code.** Collect IDs, release guards, then invoke computations. Notification from a `RefCell` write borrow or mutex guard can deadlock or panic on reentry.
7. **Use generation-checked IDs if state is scope-owned.** A copyable handle can outlive its arena slot. Disposal must invalidate the handle, and reuse must never redirect an old handle to new state. Decide whether access returns an error, cancels work, or panics with diagnostics.
8. **Restore execution context with RAII guards.** Tracking, owner, phase, and batching state must unwind correctly on errors and, where supported, Rust panics. Restoring pointers alone does not repair half-installed graph edges.
9. **Define failed-computation semantics.** Either finalize a coherent dependency set for recovery or roll back graph edits and retain explicit retry inputs. Never leave a half-connected node. An application panic policy must be separate from ordinary recoverable UI errors.
10. **Give async resources explicit inputs and generations.** Track inputs synchronously, then spawn work. On completion, verify resource generation and owner validity. Cancellation is cooperative; discarding an obsolete completion remains necessary.
11. **Keep the UI graph on a defined executor initially.** Worker tasks return messages/results. Do not imply that a cheap copyable signal handle is automatically `Send + Sync`; define cross-thread access deliberately.
12. **Define batching without promising transactions.** Specify synchronous source writes, freshness of reads, observer flushing, and frame coalescing separately. If atomic visibility or rollback is needed, introduce a distinct transaction API.
13. **Separate state from events.** Multiple signal writes can coalesce to a final value. Commands, text-input events, audit records, and deltas may require every occurrence to be delivered.
14. **Make equality policy visible.** Rust values may lack `PartialEq`; floating point includes NaN; large collections have expensive equality; shared objects can hide interior mutation. Start with explicit notification semantics and optional equality-aware setters/memos.
15. **Start without speculative reactive execution.** It adds versioned caches, multiple visible states, cancellation, resource ownership, and commit ordering. Native background work does not require speculative graph forks.
16. **Distinguish UI commit from presentation.** Updating a text node, calculating its geometry, submitting GPU work, and displaying pixels are separate completion points. Provide precisely named waiting/measurement APIs.
17. **Make resource retention observable.** Inspector counts should include sources, computations, edges, owner scopes, queued work, and pending tasks. Show why a node remains alive and why an observer reran.
18. **Keep hidden and disposed distinct.** A hidden tab may intentionally retain state, but should not accidentally keep expensive rendering or background effects active. Introduce pause/resume only with documented semantics.

**A first conformance suite**

These are acceptance scenarios for a future implementation, not tests that were run during this research.

| Test family | Required scenario |
|---|---|
| Helper transparency | Inline a tracked read, extract it into a Rust helper, and obtain identical values and dependencies. |
| Conditional dependencies | Switch between A and B repeatedly; inactive inputs stop scheduling the computation. |
| Diamond consistency | A feeds B and C; D reads both. Observers never see mixed generations after a completed update boundary. |
| Batch semantics | Multiple writes, nested batches, and a read midway through a batch follow the documented freshness rules. |
| State versus events | Three writes can yield one final UI update; three event emissions remain three events. |
| Cycles and purity | Self-recursion and multi-node feedback produce a diagnostic; derived writes are rejected. |
| Error recovery | Fail after every dependency read, change the faulty input, and verify recovery and consistent graph edges. |
| Cleanup ownership | Mount/unmount and rerun nested scopes thousands of times; drop counts and graph counts return to baseline. |
| Initialization-only reads | Read a derived chain only during setup, destroy the component, and verify a long-lived source retains nothing. |
| Handle generations | Dispose a slot, reuse storage, and verify old handles cannot read or mutate the replacement. |
| Async ordering | Resolve A/B in both orders; only the authorized generation commits. Dispose before either resolves. |
| External writes | An obsolete autosave cannot silently overwrite a newer save; choose serialization/version enforcement explicitly. |
| Cache ownership | Destroy one instance while another shares a loaded asset; the survivor and later remount remain valid. |
| Keyed lists | Insert/remove/reorder/filter/replace rows while editing; model, focus, selection, and local state follow keys. |
| Scheduling integrity | Dirty work remains scheduled across errors and cancellation; internal commit reads do not become user dependencies. |
| Thread handoff | A late worker result targets a disposed scope and is safely rejected without running UI callbacks on the worker. |
| Inspector neutrality | Enabling traces and inspecting values does not alter update results, dependency sets, or resource lifetime. |
| Scene completion | Replacing a pending scene cannot let obsolete work commit or prevent the current scene from reaching completion. |

**Recommended initial contract**

Mount components into explicit owner scopes. Use synchronous tracked computations and pure derived values, with a documented evaluation policy. Queue observers until an explicit update boundary and coalesce frames separately. Use keyed structural regions, explicit writable bindings, and resource inputs captured before asynchronous work. Support ordinary fallible application operations without treating every error as a panic. Add speculative rendering and implicit async dependency tracking only after the ownership, scheduler, and failure invariants are demonstrably sound.
