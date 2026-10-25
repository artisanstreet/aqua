# Aqua implementation plan

The user authorized building Aqua from `GUI_FRAMEWORK_DESIGN.md`, using Artisan MCP agents on Muse Spark 1.3 Contributor with requested xhigh effort. Build one shared Rust codebase, with a Windows adapter first. Other OS adapters may remain explicitly unsupported while portable headless tests run on this Linux development host.

## Integration rules

- All contributors share this workspace. Do not overwrite another contributor's files, reset changes, push, or publish. Work inside assigned files. Communicate interface changes to the orchestrator.
- Existing design documents are the specification, not proof of implemented features. Keep an honest feature ledger in README/documentation with implemented, partial and deferred states.
- Start with a working end-to-end application, then fill required behavior and test it. Real rendering, interactions, layout and lifecycle take priority over placeholder APIs. Never expose silently inert properties.
- Keep core data renderer/platform independent. Use winit, wgpu, Taffy, one Oklch color API, explicit reactive inputs, retained stable identities, demand-driven scheduling and bounded resources.
- `aqua-render` owns GPU composition; `aqua-platform` owns windows/native adapter capabilities; `aqua` integrates them. UI must be testable without a GPU or native window.
- Unsupported effects/services return explicit capabilities/errors rather than silently claiming support.
- APIs are not frozen. Agree through a written `docs/IMPLEMENTATION_CONTRACT.md` before connecting crates. Avoid incompatible speculative duplicate APIs.
- Meaningful correctness tests are required for graph disposal/batching, stale handles, incremental dirty state, layout/event handling, scheduling and feature contracts. Run formatting, workspace tests and Clippy. Check Windows target when toolchain permits; do not claim Windows execution without actually running it.

## Workstreams

1. Core/UI: scaffold workspace and shared contract first, then portable core, retained UI, layout and macro.
2. Reactivity: standalone explicit dependency graph, scopes, lifecycle, batching, derives/effects and tests; subsequent platform/integration work after graph completion.
3. Graphics: wgpu renderer and text integration, real primitives/effects/custom-pass interface and headless verification, coordinated with the core/UI contract.
4. Orchestration: platform/facade ownership allocation, Windows integration, demos, validation, independent review and follow-up fixes.

## Completion evidence

- A Rust consumer can build and run an interactive Aqua demo using the facade and `ui!`.
- Common logic is shared; Windows functionality is adapter code, not a forked framework.
- Examples exercise editing, activation, scrolling, keyed updates, overlays, animation, media and GPU extension where implemented.
- Idle scheduling and partial-update contracts have tests and observable diagnostics; no claim of beating Chromium without matched measurements.
- Final report names tests actually run, Windows cross-check/runtime status, all material unsupported capabilities and remaining scope.

The goal is broader than a toy demo. Continue implementing and reviewing after the first compile; a broad API full of no-ops does not fulfill it.
