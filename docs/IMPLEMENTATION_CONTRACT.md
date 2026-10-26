# Aqua implementation contract (core owner, living document)

Status: **checkpoint cut** — owned APIs frozen for integration. Where this
file and code disagree, **the code wins**; the owner updates this file in the
same change. `[UNIMPLEMENTED]` marks agreed-but-missing surface. Nothing here
claims an unimplemented feature works.

## 0. Toolchain, versions, commands (binding)

- Minimum supported Rust: **1.93** (required by `wgpu` 30; host is 1.98.1).
  Workspace edition 2021 for core-owned crates.
- Agreed versions: `wgpu` **30**, `glyphon` **0.12**, `cosmic-text` **0.19**,
  `winit` **0.30** (stable, NOT the 0.31 beta), `taffy` **0.15**,
  `syn` **2** (macros uses 3.0.9 for its own parser — see §7),
  `fontdb` 0.23, `unicode-segmentation` 1.12.
- Root `Cargo.toml` uses `members = ["crates/*"]` so peer crates join on
  landing; workspace deps pin `aqua-*` paths + the agreed externals.
  Root edits stay with the core owner.
- Ten agents share one host and one `target/` lock: targeted only —
  `CARGO_BUILD_JOBS=2 cargo check|test|clippy -p <owned-crate>`.
  The full `--workspace --all-targets` check and a real window are the
  parent-run integration gate before more features. Do not claim workspace
  health from excluded-module passes.

## 1. Canonical homes (single source of truth)

`aqua-core` owns ALL of these. Peer duplicates are provisional and must
become re-exports:

| Type | Canonical path | Consumers |
|---|---|---|
| `Oklch` + `srgb8_to_linear`/`linear_to_srgb8` | `aqua_core::{Oklch, ...}` | render (re-export, delete duplicate math), UI paint |
| `RawId`, `Id<T>`, `NodeId`, `ViewportId`, `ImageId` | `aqua_core::id` | ui tree, render scene store (adapt) |
| `Rect` (+`inflate`), `Point`, `Size`, `Insets`, `CornerRadii`, `Transform2D` | `aqua_core::geometry` | ui layout/hit, platform (re-export, adapter-only), render |
| `DisplayItem`, `TextRun` | `aqua_core::scene` | render store vocabulary (render owns the store) |
| `InputEvent` (+`TextInput`), `ImeEvent` (cursor range), `Key`, `Pointer*`, `ScrollDelta` | `aqua_core::event` | platform translates winit→these; ui dispatches these |
| `Role`, `SemanticNode` (+`children`, `bounds`) | `aqua_core::semantic` | ui `emit_semantics`, platform→accesskit |
| `Arena<T>`, `ArenaError` | `aqua_core::Arena` | ui tree storage |

## 2. Identity contract (binding, tested)

- Generational handles. Generations start at **0** for fresh inserts and
  bump on every removal; on counter wrap the slot is **retired permanently**
  (`retired_slots()`), never reused — no new handle can alias an old one.
- `RawId::INVALID` (`u32::MAX`/`u32::MAX`) is never issued and never resolves.
- `NodeId` identifies UI and scene nodes structurally, but UI-tree slots and
  render-scene slots are separate arenas — handles are NOT interchangeable
  between the two stores.

## 3. Color contract (binding, tested, 28 core tests green)

- One public model: `Oklch { l, c, h, alpha }` (`l` 0..=1, `c` >= 0,
  `h` degrees normalized 0..360, `alpha` clamped). Constructor `Oklch::oklch`
  keeps the CSS-function name deliberately (`#[allow(self_named_constructors)]`,
  peers already call it — do not rename).
- Inputs: `from_hex` (`#rgb/#rgba/#rrggbb/#rrggbbaa`, `Option`, never panics —
  ASCII validated before slicing), `from_srgb8`, `from_hsl` (CSS Color 4).
- Outputs: `to_gpu` (premultiplied linear sRGB, shader-ready), `to_linear`,
  `to_srgb8`, `to_hex`. `lerp` takes the shortest hue path; near-neutral
  endpoints inherit the chromatic hue.
- Conversion matrices are the published OKLab constants
  (`#[allow(excessive_precision)]`, f32 stores nearest representable).

## 4. Scene contract (vocabulary in core, store in render)

- Op vocabulary: `DisplayItem::{Rect, PushClip/PopClip, PushOpacity/PopOpacity,
  BackdropBlur, Text{at,run,color,opacity}, External{bounds,layer,opacity,
  rounded_clip}}` + `DisplayItem::bounds()` (effect footprints for damage).
- The retained incremental store (`insert/update/remove`, `drain_changes`,
  no-op-write elision, stale-handle rejection) is owned by `aqua-render`.
- `TextRun { id, revision }` is minted by `aqua-text`'s `RunRegistry`
  (see text contract `docs/TEXT.md`); UI fills `id` from the node handle and
  bumps `revision` on every content change until the registry path lands
  (explicit interim, not a second registry).
- Image layers key to the `ImageId` slot index; viewport/custom layers use
  the media owner's namespace (media3d). Renderer samples, never owns.
- Canonical layer namespace (binding, `aqua_core::scene`): `LAYER_TAG_MEDIA`
  (`0x4000…`) / `LAYER_TAG_VIEWPORT` (`0x8000…`, disjoint top two bits),
  payload `(generation << 32 | index)` with full 32-bit indices and 30-bit
  generations. Helpers `media_layer(ImageId)`, `viewport_layer(ViewportId)`,
  `decode_layer(u64) -> Option<(LayerKind, RawId)>` (unknown tags → `None`).
  NO silent masking: encoders `assert!` past `LAYER_MAX_GENERATION`
  (2^30 − 1); handle stores MUST retire slots at that bound (arena-style).
  UI emits via `media_layer`; render/media owners re-export these exact
  names and delete their duplicate tag/mask/encode items
  (`media::LAYER_TAG_MEDIA`, `viewport::LAYER_TAG_VIEWPORT`,
  `media_layer`, `viewport_layer`, both private `LAYER_PAYLOAD_MASK`s —
  wire-compatible for every encodable generation).

## 5. UI tree contract (`aqua-ui`: tree/layout/style/events — core-owned)

Mount/update/describe surface consumed by the macro-side `Element`
builders, reactive bindings, overlay host, and facade host:

- Descriptor: `NodeSpec { kind: NodeKind, style: LayoutStyle,
  key: Option<KeyHash>, children: Vec<NodeSpec> }` — plain data, no closures.
  The macro-side `Element` converts into this (macro owner owns conversion).
- Create: `mount_spec` (root replace + version bump), `insert_spec`,
  `attach`, `move_child`, `remove` (clears focus fallback, actions,
  painters, press arms; bumps versions; ancestor invalidation propagates).
- Keyed reconciliation: `sync_children` reuses nodes by stable key (equal
  keys, in-place prop updates, order follows descriptors); unkeyed children
  reconcile positionally; kind changes replace. Identity, focus, caret, and
  per-row effect scopes keyed by `NodeId` survive reorder (tested).
  Text is uncontrolled: a reused editable keeps its live model/caret/
  selection/undo (descriptor text is the mount-time default; only cosmetic
  props apply; Static<->Editable swaps backing); static labels take the
  descriptor text. Explicit values flow through `SetText` bindings.
- Update: `set_text` (editor state preserved), `set_style`, `scroll_to`
  (clamped), `set_focused` (rejects unfocusable/disabled), `set_focusable`,
  `set_semantics`, `set_canvas_painter`, `edit_text(id, closure)` (canonical
  low-level mutation), `sync_text_leaf`.
- Commands: `UiCommand::{SetText, SetStyle, SetScroll, RequestFocus, Insert
  (boxed spec), Remove, Move}`; `push_command` (safe during dispatch),
  `command_sink() -> CommandSink` (**cloneable `Rc<RefCell<Vec<UiCommand>>>`**
  for reactive effects/hosts — guards the queue only, `take()` runs no user
  code), `drain_commands() -> usize` (runs first in `compute_layout`).
- Actions: `ActionKind::{Activate, TextChanged, FocusChanged}`,
  `set_action_callback`, `fire_action` (event data only — no tree access, no
  reentrancy), `has_action`, `is_disabled` (subtree-aware).
- Press/capture: `arm_press/take_press/clear_presses` + `press_captures`
  (contact → armed node). See §8 for the exact press state machine.
- Versions/stats: `layout_version` (Taffy re-runs only on change or viewport
  change — genuine ancestor/sibling propagation preserved, paint-only frames
  skip), `content_version` (hosts gate re-emission), per-node `paint_rev`,
  `changed_paint(since)`, `FrameStats { layout_skipped, commands_drained,
  nodes_measured, nodes_traversed, scene_ops, content_version }`,
  `last_stats()`. `emit_scene(&mut self)` records op counts.
- Text backing: `TextBacking::{Static(TextModel), Editable(TextEditor)}` —
  grapheme-safe by construction; `display_text()` (preedit + password mask,
  or the `placeholder` hint when empty) feeds measure/provider; `UiTree`
  never duplicates the buffer (Taffy `LeafCtx` owns its measure copy, synced
  on content change only). `TextContent.placeholder` (default empty) shows
  only for empty editable nodes, model stays empty, `is_placeholder()` flags
  muted rendering, IME preedit takes precedence; static labels ignore it.
- Text measure: `TextMeasurer` trait; `NaiveMeasurer` (tests/bootstrap only);
  `CosmicMeasurer { fonts: SharedFonts, scale }` over the shared font manager
  (same instance the renderer uses — measurement/pixels cannot disagree).
- Canvas: `CanvasContent { painter }` + registry; painter `Fn(Rect) ->
  Vec<DisplayItem>`, no tree access. No painter → space reserved, nothing
  emitted (honest, pending render custom-pass contract).
- Semantics: `NodeSemantics` (default `Group` — correct for containers,
  WRONG for controls; leaving `Group` on a button is a contract violation),
  `emit_semantics() -> Vec<SemanticNode>` (children, bounds, role, disabled,
  focus, label with text fallback, value).
- UI crate never depends on render/platform/winit/wgpu — headless testable
  (81 ui tests green). `overlay`/`motion`/`overlay_host` are peers' files in
  this crate; `element`/`binding` are macro-owned (mod lines added by macro
  agent — preserved, never rewritten by core).

## 6. Reactive↔UI binding contract (reactive complete, 27 tests)

- Primitives (actual names, `docs/REACTIVITY.md`): `Runtime::new`,
  `create_scope`, `Scope::signal -> (Read, Write)`, `derive`/`derive1`
  (explicit tuple inputs, 1–8), `subscribe` (the binding primitive),
  `effect`, `on_cleanup`, `batch`/`flush`, `set_auto_flush`/`set_wake_callback`,
  `pending`/`stats`/`retention`. Handles are `Weak`; Runtime holds strong.
- Canonical low-level binding: a plain Rust effect subscribing with a target
  `NodeId` calls `tree.edit_text`/`set_text`/`set_style`/… or pushes
  `UiCommand`s into a captured `CommandSink` — mount-once targeted slots,
  never a whole-subtree rebuild per signal. Documented `Text(bind:
  (ReadSignal, WriteSignal))` shape is macro/facade sugar over this path
  (macro owner).
- Keyed collections: `sync_children` preserves `NodeId`s; per-row scopes are
  owned by effect owners keyed by `NodeId` and disposed on removal (tree drops
  actions/painters/arms with the node; scope disposal is the reactive side).
- No `RefCell` tree reentry: callbacks receive data only; effects run at the
  update boundary; `drain_commands` applies outside dispatch.

## 7. Macro contract (`ui!` — macro owner, 14 tests)

- Codegen targets `#crate_path::ui::{Box,Text,…}::builder()` with `.arg()`,
  `.{prop}()`, `.child()`, `.build()`; `Fragment::builder/from_nodes`;
  `Element.with_key`. Default crate path `::aqua` (facade re-exports
  `aqua_ui` builders as `::aqua::ui`).
- Builtins provided by UI today: `Box Text Media Scroll Canvas Viewport3D
  Fragment`. `Button Checkbox Slider Dropdown Tabs Dynamic` have NO builder
  yet — using them fails at the builder (honest, until owners land them).
- Macro crate has no runtime dependency (no cycle). `element.rs`/`binding.rs`
  exist in `aqua-ui/src` (macro-owned); their `pub mod` lines are added by
  the macro agent and preserved by all other editors of `lib.rs`.

## 8. Input contract (binding, tested incl. press sequences)

- Platform routes winit events → core types: `TextInput { text }` exactly
  once per key event, SUPPRESSED during IME (IME text via `Ime::Commit`);
  `Key::Char` is single-scalar identity only (multi-scalar via `TextInput`);
  `ImeEvent::Update { marked, cursor: Option<(usize,usize)> }` carries byte
  cursor; `\r\n`/`\r` normalize to `\n` at dispatch.
- Press state machine: primary down arms nearest activatable ancestor
  (label hits bubble via `has_action` walk; disabled subtree blocks) with
  per-contact capture + focus grant; primary up inside bounds fires
  `Activate` once; release outside, cancel, focus loss, or removal clears
  without firing; non-primary buttons never arm/fire. Stale ups after removal
  are unhandled (false), never fired.
- Keyboard: Tab/Shift-Tab cycles focusable+enabled nodes (wrap); Enter/Space
  activates focused non-editable controls (disabled blocked); `KeyDown` never
  inserts text (prevents double-insert with `TextInput`); IME Commit folds
  preedit OR inserts fallback text (never both — regression-tested).
- `dispatch_input` returns consumed-ness; `PointerMove` hover unimplemented
  (returns false; drag-out decided at release).

## 9. Platform contract (platform owner, 44 tests, `docs/PLATFORM.md`)

- Translates native→core events (see §8 routing rules), reports capabilities
  explicitly (unsupported = typed, never silent success), converts
  `emit_semantics()` to native accessibility (role/label/bounds/focus/
  disabled/value — no setting-role-alone behavior).
- Owns `geometry.rs` as adapter-only; canonical geometry is core's.

## 10. Open coordination items (owners in parentheses)

1. Render: re-export core `Oklch`/`DisplayItem`/IDs/geometry; delete duplicate
   math; confirm `AuthoredColor→Oklch`, `h_deg→h` renames.
2. Text: `RunRegistry` mint path replacing UI interim run ids; `measure_text`
   single-line mode (currently forces Multi — UI passes `max_width: None`
   for single-line as workaround); password-mask coordinate note.
3. Macros: `Element→NodeSpec` conversion; `Text(bind:…)` sugar; pending
   `pub mod element/binding` lines (theirs to add, all preserve).
4. Facade: `UiApp` host on the real tree (`mount_spec`, `CommandSink`,
   `UiCommand` drain, `FrameStats` gating, `emit_scene`/`emit_semantics`,
   press/key dispatch); duplicate scheduler/reactive shims removed in favor
   of real peers (parent-directed).
5. Render+media3d: custom-pass contract for Canvas painters, `External.layer`
   namespace, Viewport3D GPU extension contract (device/queue, surfaces,
   resize/device-loss, focus-scoped input).
6. Overlay host (`overlay_host.rs`, overlays agent): popups as visual-root
   children with logical ownership in the overlay service.

## 11. Non-goals at checkpoint

No new features until the parent-run `--workspace --all-targets` check and a
real window pass. Known honest gaps: single-line `measure_text` forcing
Multi; UI interim text-run ids; Canvas output pending render custom passes;
hover unimplemented; overlay host wiring in progress (peer).
