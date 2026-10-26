# Collection runtime API (runtime owner → driver owner + parent)

Frozen seam lives in `crates/aqua-ui/src/collection_runtime.rs` (single
source; `collection.rs` re-exports). The View driver (`binding.rs`,
facade worker 01a178ec) consumes exactly the API below. No `binding.rs`
or `tests/collection_runtime.rs` edits here — those are the driver's.

## 1. Frozen seam types (unchanged signatures, contract §"Frozen internal seam")

```rust
pub struct CollectionLayout { pub content_width: f32, pub column_widths: Vec<f32> }
pub enum CollectionRole { List, Table }
pub trait CollectionSource {
    fn len(&self) -> usize;
    fn key(&self, index: usize) -> Key;                    // runtime OVERWRITES row keys with this
    fn layout(&self, available_width: f32) -> CollectionLayout;
    fn row(&self, index: usize, layout: &CollectionLayout) -> Element;  // NO keys set inside
    fn header(&self, layout: &CollectionLayout) -> Option<Element>;
    fn role(&self) -> CollectionRole;
    fn validate(&self) -> Result<(), CollectionError>;
}
pub struct CollectionDescriptor {
    pub source: Rc<dyn CollectionSource>,
    pub styles: Vec<Style>,
    pub virtualized: bool,
    pub estimated_row_height: f32,   // retained RAW; non-finite/<=0 fails validation
    pub overscan: usize,
}
impl CollectionDescriptor {
    pub fn new(source, styles, virtualized, estimated_row_height, overscan) -> Self;
}
```

`CollectionError`: `MissingRowFactory | MissingKeyFactory | DuplicateKey |
DuplicateColumnId | InvalidSize(String) | InvalidTrack(String) |
Validation(String)` — `Debug/Clone/PartialEq/Display/Error`.

`validate_snapshot(&dyn CollectionSource)` — full `O(n)` key-uniqueness
pass, no truncation (structural refreshes only, never per scroll).
`validate_descriptor(&desc)` — estimated height must be finite `> 0`
else `InvalidSize` (NO silent defaulting, per parent P0).

## 2. Element API (done, in `element.rs`)

- `Element::collection(CollectionDescriptor) -> Element` — lowers to the
  SHELL (below). No row/header/layout factory calls during lowering.
- `Element::role(Role) -> Element` — explicit semantics; fragments
  propagate to children lacking one (also covers later-appended children
  at splice/lower time).
- `Element::collection_semantics(CollectionSemantics) -> Element` —
  coordinates override; fragments propagate likewise.
- `.children(impl IntoIterator<Item = impl Into<Element>>)` on ALL
  child-carrying builders (`Box Text Media Scroll Canvas Viewport3D Button
  Checkbox Slider Dropdown Tabs Dynamic Fragment`) plus `Element::child` /
  `Element::children` on built elements. Repeated calls append.
- `LowerError::{CollectionChildren, Collection(CollectionError)}` added.

`pub(crate)` for the driver: `role_opt()`, `collection_semantics_opt()`,
`is_collection()`, `collection_descriptor()`.

## 3. Agreed shell (lowering output — driver builds into it)

`shell_shape(&desc) -> ShellShape::{ListBody, TableSplit}` (`TableSplit`
for ALL tables, both modes; lists are `ListBody`).

- List virtualized → `Scroll` root `[managed]`, empty children.
- List normal → `Box` column root `[managed]`, empty children.
- Table BOTH modes → `Box` column root `[managed]`, empty children
  (never `Scroll`: header clips wrong). Driver creates
  `[header_clip_Box, body_Scroll]` children in order in BOTH modes; header sits OUTSIDE the
  vertical body viewport and mirrors body horizontal offset.
- Virtualized + authored width/height `Auto` → `Pct(100)` fill (bounded;
  unbounded parents fall back to the UiApp viewport in the driver).
- All shell nodes `collection_managed = true` (both modes): `update_spec`
  applies style/paint only — never clobbers live scroll from stateless
  specs, never re-syncs children. `refresh_mounted` refreshes container
  style/paint and returns early (both modes).
- `NodeSpec.collection_managed: bool` (`tree.rs`); `UiTree::scroll_offset`
  getter; keyed `sync_children` fixed from `O(n²)` → `O(n)`
  (index map + draining iterator, semantics preserved).

Driver structural rule: on snapshot change verify shell shape; on shape
change (list virt flip) re-realize from scratch (remount path already
handled by discriminant: virtualized list = 3, else 0).

## 4. Driver helpers (all in `collection_runtime.rs` — keep, do not duplicate)

- `container_style(&desc) -> (LayoutStyle, Paint)` — ordered Style merges.
- `body_gap(&desc) -> f32` — authored inter-row gap (`row_gap` else `gap`).
- `INITIAL_AVAILABLE_WIDTH = 800.0` — first-plan guess only.
- `lower_row_specs(source, index, plan, inherited)` — row Element with
  runtime-forced key → specs. Exactly-once factory calls live here.
- `lower_header_specs(source, plan, inherited)` — unkeyed positional.
- `spacer_spec(height)` — plain geometry box, zero margins.
- `ReadingAnchor{key, offset_in_row}::restore_scroll(sums, index, vh)`.

## 5. `CollectionState` (index math the driver drives)

Stride model: `stride[i] = height[i] + margin_top[i] + margin_bottom[i] +
gap`. Prefix sums over strides (Fenwick, `O(log n)`). Internal body stack
Taffy gap forced to 0 by the driver; the authored gap is folded into
strides (spacers are ordinary siblings and would otherwise inherit a
phantom gap and drift). Positions match a full layout exactly.

- `new(desc)` / `refresh(desc)` — full `O(n)` validation + height/margin
  preservation by key + anchor re-resolve (scroll moves with anchor key).
- `set_viewport(w, h) -> bool` (plan changed?), `plan()`.
- `set_scroll(x, y)` (finite-only latch), `scroll()`.
- `desired_window() -> RowRange` — visible + overscan; zero viewport →
  minimal window, never all. `mounted_runs(window)` — window + isolated
  pinned run (pin never drags the gap). `set_mounted()` / `mounted()`.
- `note_measured(i, h, mt, mb) -> bool` — exact equality; zero heights OK;
  non-finite/negative rejected. Shifts `scroll_y` by delta when the row is
  entirely above the viewport top (anchor preserved on above-viewport
  height changes, not just refreshes).
- `span_height(s, e)` — spacer height = stride sum minus one gap.
  `offset_of(i)`, `total_height()`, `gap()`, `restyle_gap()`.
- `set_pinned_key(Option<u64>)` / `pinned_key()`, `index_of(key) -> O(1)`.
- `capture_anchor()` / `anchor()`, `update_styles(styles)` (style-only,
  preserves everything else), `heights_dirty()` / `mark_settled()`,
  `len()` / `is_empty()` / `descriptor()`.

## 6. Effective viewport rule (driver implements)

- Width: body committed rect width if finite and `> 0`, else UiApp
  viewport width.
- Height: body committed rect height IF constrained
  (`rect_h < total stride estimate`), else UiApp viewport height.
  (Unconstrained Auto bodies expand to content; clamping to UiApp bounds
  the window: root virtual list, no explicit height, at 600×400 mounts
  ~30 rows, never 100k.)
- Table horizontal: body is canonical; header clip Box mirrors `body.x`
  (margin/content shift — driver detail); header `y` forced 0.
  Windowing viewport height uses BODY rect only.

## 7. Semantics (driver applies via `tree.set_semantics`)

`NodeSemantics.collection: Option<CollectionSemantics>` added (default
`None`); `emit_semantics` propagates. Driver sets:

- List container `Role::List` + `{set_size: len}`; rows `Role::ListItem` +
  `{position_in_set: i+1 (1-based), set_size: len}`.
- Table container `Role::Table` + `{row_count, column_count}`; body rows
  `Role::Row` + `{row_index}`; header container `Role::Row`; header cells
  `Role::ColumnHeader` + `{row_index: 0, column_index}`; body cells
  `Role::Cell` + `{row_index, column_index}` (cell roles come from source
  Elements via `Element::role`; runtime sets container/row/header).
- Preserve existing label/disabled/selected/value_text (read-modify-write).

## 8. Verification (runtime owner, this session)

- `cargo check -p aqua-ui`: my files clean. `collection.rs` had API-owner
  mid-edit errors (not mine); latest check passes lib-wide.
- `cargo test -p aqua-ui --lib`: 175 passed, 0 failed — incl.
  `virtualization::tests` (Fenwick exactness, 100k bounded windows, large
  jumps, zero-viewport minimal, point updates), new `element::tests`
  (shell-only lowering with ZERO factory calls incl. tables both modes,
  shell shapes, managed flags, fill default, duplicate keys, bad policy,
  role/fragment propagation, style merges, children sugar), new
  `tree::tests` (collection metadata round-trip) + all pre-existing
  suites (keyed sync, scopes, editing, paint-skipping at scale).
- `cargo test -p aqua-ui` integration: `collection_builders` (API owner)
  has 2 failures at last look — theirs, not runtime. `overlay_motion`
  + `style_builder` pass.

## 9. Known limits (no claims beyond headless evidence)

- No native screen-reader verification (headless tests only).
- Vertical wheel gestures directly over the table header strip: header is
  a non-scroll Box, so `dispatch_scroll` does not consume; hosts must
  forward unconsumed events to the body scroll (document for facade).
- Unbounded-width tables: plan falls back to `INITIAL_AVAILABLE_WIDTH`
  until a real viewport commits; first frame may rebuild rows once.
- `shell_shape` positions (`children[0]` header, `[1]` body) are
  positional by construction; driver must re-discover after remount.
- `overlay_host.rs:678` destructuring touched (`..`) to cover the new
  `NodeSpec` field — overlays owner please confirm.
