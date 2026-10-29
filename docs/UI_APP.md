# UiApp (`aqua/src/ui_app.rs`) + `examples/todo.rs`

Owner: overlays/UI-host agent. Status: **implemented, frozen**
(workspace `cargo check --all-targets` green, strict `clippy -D warnings`
clean for owned files, `cargo test -p aqua` green incl. verifier
consumer tests, `cargo run -p aqua --example todo -- --headless` green
end-to-end, native Win11 todo EXE smoke EXIT 0 — see Verification).

## What it is

`UiApp` is the single-window high-level host: one `UiTree`, one
`ReactiveRuntime` + root `Scope`, one `View` mount, shared fonts/run
registry, the `OverlayHost` (which owns motion), and the retained
`Scene` reconciliation — implementing `App`, so an element factory becomes
a runnable program via `Aqua::run_ui` / `Aqua::run_ui_reactive`.
One window per `UiApp`; the runtime stays the multi-window low-level
owner. No competing text, overlay, or graph logic.

## API (frozen)

- `UiApp::new(fonts, runs)`, `mount(factory) -> Result<_, UiAppError>`
  (rejects second mount with `AlreadyMounted`), `mount_reactive(deps,
  render)` (explicit-dependency structural seam; effects render into a
  queue the frame boundary consumes — effects never touch the tree),
  `refresh(elements)`, `sync() -> Result` (flush + structural drain),
  `layout()`, `handle_input(&InputEvent) -> bool`, `build_frame(&mut
  Scene) -> Result<Option<Instant>, UiAppError>`, `build_scene_into(&,
  &ObservedWindowState) -> Result<Option<Instant>, String>`,
  `build_error()`, `frame_stats()`, `ime_anchor()` (+ `App::ime_cursor_area`
  override delegating to it), `on_a11y_action` routing (Focus moves real
  focus; Press focuses + fires the real `Activate`; disabled nodes skip;
  `TextSelection` data applies via editor `set_selection` with char→byte
  clamping), `set_externals(provider)` (GPU boundary, called from
  `App::externals`), `text_run_count()`, `dispose()` (idempotent) + `Drop`,
  `run_ui` / `run_ui_reactive` on `Aqua`.
- Keyed reconciliation preserves nodes/focus/carets across structural
  rebuilds; targeted prop updates stay in bindings (never a rebuild).
- Text: unmasked shaping source (`shaping_source`: editor display text
  incl. preedit / static model text / placeholder hint) + `set_mask`
  keeping source coordinates; cache key = display-text hash + layout
  inputs (never just the committed rev). Placeholder is never masked;
  core `display_text()` masking is for measure only.
- Scene reconcile by `(NodeId, op index)` keys (+ caret/selection/IME
  keyspaces); identical order → in-place `update` (renderer no-ops);
  structural change → ordered rebuild. Media layers come from core's
  canonical `media_layer` (no UiApp remap).
- Modal `TextInput` respects the focus trap: allowed only inside the
  topmost modal subtree or a popup stacked above it (mirrors pointer
  policy); otherwise consumed-and-dropped.
- Pointer caret: press on the focused editable sets the caret via shaped
  `layout.hit` (grapheme-snapped). Drag-select/word-click unimplemented
  (documented gap; `set_caret`/`set_selection` from text owner unblock it).

## Frame flow (`build_frame`)

flush → structural drain → `sync_lifecycle` → layout → `reanchor_all` →
text-run sync (+GC of removed runs) → emit (interim runs → real
`RunId`/rev) → caret/selection/IME items from resolved layouts →
reconcile → motion advance → relative deadline
(`motion_deadline_duration`, never elapsed-doubling).

## Example (`examples/todo.rs`)

Windowed by default (`cargo run -p aqua --example todo`);
`--headless` runs the deterministic pump: mount → type into the bound
field → Add (real down+up press) → toggle via checkbox → Remove via
button → asserts each step (rows, header counts, scene ops, text runs,
`build_error() == None`).

## Verification (final gates)

- `cargo check --workspace --all-targets`: green.
- `cargo test -p aqua`: all suites green (incl. keyed structural tests
  and verifier `tests/ui_app.rs` consumer tests + strict-GPU artifact
  `ui_app_todo_artifact_real_text`). Verifier `chunks_exact` lint: FIXED.
- `cargo clippy -p aqua --all-targets -- -D warnings`: clean for owned
  files (`ui_app.rs`, `todo.rs`).
- `cargo fmt -p aqua -- --check`: clean for owned files.
- `cargo run -p aqua --example todo -- --headless`: full row lifecycle
  green (mount → type → Add → toggle → Remove, `build_error: None`).
- Native Win11 todo EXE (parent run): EXIT 0, ~2 s smoke —
  wakeups 5, requested 4, produced 2, presented 1, second frame skipped
  clean, `render_errors` 0, system clipboard. High-level native GPU run
  proven; no appearance/AT verification (see gaps).

## Actual gaps (no code edits — sources frozen for final gates)

1. **Drag-select / word-click**: pointer caret sets a collapsed caret;
   gesture disambiguation (drag to extend, double-click word) is
   unimplemented. `set_caret`/`set_selection` landed; no fake behavior.
2. **Native AT untested**: `on_a11y_action` routes Focus/Press/TextSelection
   against the real tree, but no native assistive-technology run has
   exercised it — Windows AT verification pending (appearance/AT were
   explicitly out of the smoke scope).
3. **Linear host traversals**: `sync_text_runs`, `caret_selection_items`,
   and `overlay_of_root`-style walks are O(tree) per frame. Fine for the
   todo scale; a dirty-set/incremental pass is the known next perf step
   if profiles demand it — not correctness debt.
4. **Clipboard/shortcut integration**: system clipboard proven present by
   the smoke run; app-level clipboard shortcuts (copy/paste/select-all
   key bindings in fields) are not wired — routing exists
   (`dispatch_text_input`, `set_ime_cursor_area` path), bindings pending.
