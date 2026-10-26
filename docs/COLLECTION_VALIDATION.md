# Collection validation

20 black-box tests through the real `UiApp` frame pipeline
(`mount` / `refresh` / `sync` / `build_scene_into` / `handle_input`,
retained `Scene`, `UiTree` rects/scroll/semantics, headless GPU readback).
Public `Aqua` factory APIs only — no shims, no fake rows.

| # | Test | Contract requirement |
|---|------|----------------------|
| 1 | `unvirtualized_list_mounts_every_row` | OFF mounts every item via the same row factories |
| 2 | `unvirtualized_table_mounts_real_headers_and_all_cells` | Real header elements; cell/header factory accounting; body rows stack vertically at line height; header visible (height, order, in-root) |
| 3 | `virtualized_list_100k_bounds_factory_calls_and_mounted_nodes` | Virtual ON: visible+overscan only for 100k rows |
| 4 | `virtualized_table_bounds_cell_factories_and_keeps_headers` | Same for tables; headers stay mounted |
| 5 | `scroll_and_resize_move_the_virtual_window` | Real `scroll_to` + `InputEvent::Scroll` jumps; resize changes range |
| 6 | `variable_heights_and_prepend_preserve_reading_anchor` | Measured heights; prepend keeps stable key + within-row anchor |
| 7 | `keyed_reorder_preserves_identity_and_edit_focus` | Reorder keeps node identity, focus, edit content |
| 8 | `focused_far_row_pinned_alone_not_entire_gap` | Focused row pinned in isolation; gap not mounted |
| 9 | `scroll_churn_stays_bounded` | O(window) work per scroll; nodes/runs GC'd; head restorable |
| 10 | `table_shares_one_width_plan_and_styles_merge` | Shared header/body widths (matched 6px chrome per the frozen chrome-match rule); joint h-scroll; finite root `styles` bounds + `header/row/cell_styles` normal merges; painted cells in scene; header visible (height, order, in-root) |
| 11 | `duplicate_row_keys_surface`, `duplicate_column_ids_surface`, `invalid_tracks_and_heights_surface` (neg `fr`, bad estimate, `min>max`), `missing_factories_surface` (no row / no key / no columns) | Invalid inputs fail loudly, never silent or panic |
| 12 | `semantics_carry_roles_totals_and_logical_indices` | `ListItem`/`Row`/`ColumnHeader`/`Cell` roles; one-based list positions; zero-based table indices; totals via `CollectionSemantics` |
| 13 | `collections_paint_real_pixels_offscreen` | Strict GPU pixels for List+Table; only `AdapterNotFound` may skip (strict iff `AQUA_REQUIRE_GPU=1`); PNG iff `AQUA_PNG_OUT` set |
| 14 | `live_signal_subscriptions_survive_window_updates_and_dispose_on_eviction` | Signal-backed rows stay subscribed across small scroll + resize (catches cached-binding disposal bug); eviction disposes scopes/effects; ghost writes queue nothing; dispose returns effects/sources to baseline and scopes to 0 (root itself disposed) |
| 15 | `nested_rows_inherit_typography_and_header_button_activates_on_press` | Rows inherit parent `text_color`/`font_size` through the collection boundary (tree + painted scene); header `Button` runs app code via a real press at visual coords |
| 16 | `table_plan_correct_on_first_frame_then_stable_without_refactory` | First frame at actual 700px already uses the 700px plan (not the 800px seed); steady frames reinvoke no factories |
| 17 | `virtualized_table_body_stacks_and_header_stays_fixed_on_vscroll` | Virtualized body rows stack vertically at line height; header stays fixed while the body scrolls |

Deliberately not mirrored 1:1: `min/max_width` happy paths, `pct` tracks,
`column_widths()` math (builder-owner unit territory), `estimated_row_height`
valid values, `overscan(n)` setter values — covered representatively (test 11
rejects bad bounds; tests 3–5 bound the default overscan window).

## API basis and adaptations

- Single source: `aqua_ui::collection` via the `Aqua` facade, as landed.
- `ColumnBuilder::header` takes `impl Fn() -> Element` (closure).
- Exact `CollectionError` variants are NOT matched: invalid-input tests
  assert generic loud surfacing (`mount` `Err`, frame `Err`, or
  `build_error().is_some()`), never silent omissions or panics.
- `Table` container `styles()` set finite root bounds matching the viewport
  in every table test. Header/row horizontal chrome (gap, L/R padding, L/R
  margins) is identical on both sides per the chrome-match rule.
- Position assertions and synthetic press coordinates use screen space via a
  `visual_rect` helper (layout rect minus ancestor `Scroll` offsets,
  mirroring emission/hit-test); `UiTree::rect` is layout-space by design.

## Run evidence

Final freeze result — run 7 (2026-10-26,
`AQUA_REQUIRE_GPU=1 --test-threads=1 --nocapture`): **20 passed, 0 failed.**
Headless adapter present (llvmpipe/Vulkan — strict GPU path executed, no
skip). Covers the `assert_header_visible` regression (visual height > 0,
header/body non-overlap, header-in-root in tests 2/10/17; press-point guard
in test 15) and the vertical-stacking asserts (tests 2 + 17). Driver fixes
verified by the suite without weakening any assertion: body `Column` axis
(stacking), header visibility, horizontal header/body scroll sync.

### Historical runs

- Runs 5/6 (same flags): 18–19 passed. Remaining failures were the
  `live_scopes` baseline (test bug, fixed: the root app scope counts as live
  at baseline, so dispose targets 0) and table horizontal header sync (real
  defect, since fixed).
- Run 3 (2026-10-26, same flags): blocked before execution — driver
  mid-edit. `aqua-ui/src/binding.rs` failed to compile (`E0425 cannot find
  value 'dropped'` at binding.rs:754; `E0599 NodeKind has no .clone()` at
  binding.rs:857). Reran once the driver signaled green.
- Run 2 (2026-10-26, same flags): 14 passed, 5 failed (up from 7/12).
  Test-bug fixes verified (reorder caret, semantics descendant labels,
  table shell now mounts). Remaining 5 were relayed to the driver as real:
  focused-row pin, live-signal resize retention, Scroll-event dispatch,
  table-10 h-scroll delta, prepend anchor (+640 exact) — all since fixed.
- Early `cargo check -p aqua --tests` (2026-10-26): workspace did not
  compile — peer mid-edit breakage, none of it referencing this suite.

`rustfmt --edition 2021 --check`: clean.

## Findings (all resolved during the freeze)

Seam duplication replaced by re-exports; `ColumnWidth` canonical with
working facade re-exports; closure headers; `Table::descriptor()` forwards
container `styles`; runtime realization landed (window sync, keyed
reuse/anchor/pin/disposal, validation wiring, semantics propagation);
`sync_children` sets fresh record parents (fixed pin, anchor, and scroll
ancestors); virtual `Auto` width renders as `Pct(100)`; body `Column` axis
(stacking); header visibility and position; horizontal header/body scroll
sync; focused-descendant removal fallback.

## Freeze

Suite frozen at 20/20. Parent takes gates after driver freeze.


## Final workspace evidence

Strict canonical validation passes all 11 checks with no failures or GPU skips.
The facade macro feature suite passes 21 tests. Windows release Todo and
Collections executables both launch, present, and exit 0 with no render errors.
See [`VALIDATION.md`](VALIDATION.md) for the final logs and performance limits.
