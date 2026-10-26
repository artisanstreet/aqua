# Collections: `List<T>` / `Table<T>` / `Column<T>`

Canonical collection authoring lives in `aqua_ui::collection` (the facade
`Aqua::List` / `Aqua::Table` / `Aqua::Column` / `fr` forwards to it without
substitution). This guide is for application authors; the frozen
builder/runtime seam is defined once in `aqua_ui::collection_runtime` and
re-exported from `aqua_ui::collection`.

## Lists

```rust
use aqua::{Aqua, px};

let el = Aqua::List()
    .items(rows)              // arrays, Vecs, lazy iterators; snapshotted
    .key(|r| r.id)            // stable key (K: Into<Key>); required
    .row(|r| Aqua::Text(r.label.clone()).build())
    .styles([Aqua::Style().gap(4.0)])
    .build();                 // Element::collection (runtime realizes it)
```

- `.virtualize()` opts in to lazy realization and is OFF by default.
- `.estimated_row_height(px(32.0))` (default 32) and `.overscan(4)` (default
  4 rows per side) seed the virtualized window.
- The row factory runs only for mounted rows — never in bulk at build time.
  Unvirtualized mode mounts every row through the same factory.

## Tables

```rust
use aqua::{Aqua, fr, px};

const NAME_TRACK: aqua_ui::collection::ColumnWidth = fr(2.0);

let el = Aqua::Table()
    .items(rows)
    .key(|r| r.id)
    .columns([
        Aqua::Column("id")
            // NOTE: header is a factory — wrap elements in a closure.
            .header(|| Aqua::Text("ID").build())
            .cell(|r| Aqua::Text(r.id.to_string()).build())
            .width(px(120.0))
            .build(),
        Aqua::Column("name")
            .header(|| Aqua::Text("Name").build())
            .cell(|r| Aqua::Text(r.label.clone()).build())
            .width(NAME_TRACK) // or fr(2.0) inline
            .build(),
    ])
    // Container (viewport bounds live here, e.g. bounding the box the
    // table virtualizes inside):
    .styles([TABLE_BOX])
    // Normal Style merges for each part:
    .header_styles([Aqua::Style().background(INK)])
    .row_styles([Aqua::Style().background(PAPER)])
    .cell_styles([Aqua::Style().padding(px(6.0))])
    .build();
```

- `Aqua::Column(id)` takes the id first; `.build()` completes to the
  uniform `Column<T>` (heterogeneous cell closures erase to one type, so
  arrays work). `Element` is not `Clone`, so headers are factories
  (`.header(|| element)`); the runtime rebuilds the header per width plan.
- `fr(..)` is a `const fn` returning the plain `ColumnWidth` enum, so static
  tracks stay `const` alongside a `const Style`. Invalid weights are stored,
  never panicked on — source validation rejects them.
- Header, every body row, and horizontal scroll share ONE width plan for the
  current available width; the runtime mirrors one horizontal offset.

## Track placement (authoritative)

Shared widths alone do not align columns, so the track owns placement
through a shared row-chrome plan:

- Row containers (header bar, body rows) must be plain row flex boxes: axis
  row, no wrap, justify start. Anything else fails validation loudly.
- Horizontal chrome — left/right padding, left/right margin, effective
  column gap — must MATCH between `header_styles` and `row_styles`. Only
  incompatible differences fail (e.g. a header gap of 8 vs a body gap of
  10); a shared nonzero gap/padding is normal styling. The common inset
  `padL + padR + gap * (cols - 1)` is reserved up front: tracks allocate
  against `max(0, available - inset)` and
  `content_width = sum(widths) + inset`, so header, body, and scroll agree.
- Vertical chrome — top/bottom padding and margins, row gap — is fully
  independent per part, as are fills, radii, and typography.
- Outer cells are `flex_shrink(0)` at the shared widths (narrow viewports
  overflow into scroll instead of squeezing tracks). No left/right margin,
  no absolute positioning, no width/min/max/flex-grow overrides in
  `cell_styles`; cell padding (all sides) is border-box inside the track,
  and cell top/bottom margins are allowed.
- Fractional flex widths are snapped to whole px by the layout engine,
  identically for header and body (header↔body equality is exact; plan
  comparisons allow 1px).

## Validation

Missing key/row/cell factories, duplicate keys/column ids, invalid
sizes/tracks/bounds, track-breaking styles, and non-`px` estimated heights
all fail `validate()` (builders and sources) and the runtime's mount/layout
path loudly — rows are never silently omitted and nothing panics. Estimated
heights keep their original value end to end so explicit validation sees (and
rejects) the invalid input instead of a silent default.

## Semantics, sorting, sizing

- List rows wrap the arbitrary row element in an ordinary structural `Box`
  carrying `ListItem`, so inner controls keep their real roles (a `Button`
  row stays a `Button`); table rows/cells carry `Row` / `ColumnHeader` /
  `Cell` on their dedicated row/cell boxes;
  logical positions and totals are assigned by the runtime.
- Ordering/sorting stays application-owned: feed items pre-sorted.
- There is no fit-to-content: sizing uses explicit `px`, `pct`, and proportional `fr` tracks, never an implicit scan of virtual rows.
