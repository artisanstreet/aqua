# Styling with `Style`

Canonical authoring API: `aqua::Style` (single source: `aqua-ui::style`;
facade re-exports it at the crate root and in the prelude). Normative
contract: [`STYLE_CONTRACT.md`](STYLE_CONTRACT.md).

## The model

`Style` is a **partial** style: every field starts **unset**, and only
explicitly set fields affect resolution. All fields are private;
canonical construction uses `Aqua::Style()` (`const fn`).

```rust
use aqua::{Aqua, Style, auto, pct, px};
use aqua_core::Oklch;

const BASE: Style = Aqua::Style()
    .width(px(1024.0))
    .height(px(800.0))
    .padding(px(16.0))
    .radius(px(12.0))
    .background(Oklch::WHITE)
    .text_color(Oklch::BLACK);
```

`LayoutStyle` + `Paint` stay as the lower-level resolved (total)
compatibility types — the tree, layout (`to_taffy`), and scene emission
keep reading them unchanged. Convert explicitly at the boundary:

```rust
let resolved = Style::compose(&[BASE, override_]);
let layout = resolved.resolve_layout();   // LayoutStyle
let paint = resolved.resolve_paint();     // Paint
let typo = resolved.resolve_typography(); // Typography (14.0 / WHITE / false)
```

## Using the same styles from Rust and `Aqua::*()`

```rust
use aqua::{px, Aqua, Oklch, Style};

const PANEL: Style = Aqua::Style()
    .padding(px(16.0))
    .radius(px(12.0))
    .background(Oklch::new(0.18, 0.02, 250.0))
    .text_color(Oklch::WHITE);

let direct = Aqua::Box()
    .styles([PANEL, Aqua::Style().width(px(1024.0)).height(px(800.0))])
    .child(Aqua::Text("Hello, Aqua").build())
    .build();
```

A single `.style(PANEL)` applies one partial style. Styles also accept
borrowed arrays or slices; repeated `.styles(...)` calls apply in call
order, including when interleaved with direct element setters.

## Lengths and spacing (frozen const chain)

- `px(v)` / `pct(p)` / `auto()` are `const fn` returning exact `Length`.
- Sizes are exact-`Length` and `const`: `width`, `height`, `min_width`,
  `min_height`, `max_width`, `max_height` each take `Length`.
- Uniform `padding` / `margin` / `radius` take px-only `Length`:
  `Pct`/`Auto` are rejected (panic) rather than silently reinterpreted —
  the old lossy `Pct → px` / `Auto → 0.0` conversions are gone.
- Per-edge spacing goes through `padding_insets(Insets)` /
  `margin_insets(Insets)` (`Insets::uniform` / `Insets::hv` are `const`).

## Colors in const position

`Oklch::new(l, c, h)` (opaque) and `.with_alpha(a)` are `const fn`, so
real theme paints can live in `const` styles — no hex parsing at runtime:

```rust
const INK: Oklch = Oklch::new(0.1991, 0.0222, 260.47); // #101620
const CARD: Style = Aqua::Style().background(INK).radius(px(12.0));
```

`Oklch::WHITE` / `BLACK` / `TRANSPARENT` are also `const`.

## Merge rules: later wins, per field

`merge` / `with` / `compose(&[...])` fold in order; only explicitly set
fields (including explicit-none from `clear_*`) overwrite. Unset fields
never overwrite anything:

```rust
use aqua::Style;
use aqua_core::Oklch;

let s = Style::compose(&[BASE, override_]); // override_ wins per set field
let cleared = Aqua::Style().background(c).clear_background(); // fill -> None
```

Optional effects distinguish **unset** (inherits / leaves the target
alone) from **explicit none** (actively clears): every `clear_*` setter
(`clear_background`, `clear_border`, `clear_opacity`,
`clear_align_self`, `clear_grid_columns`, …) records explicit-none, and
merging preserves the distinction.

## Fixed arrays are values, not `const` items

`[BASE, override_]` is a fixed-size array **value** whose length is known
to the compiler. Its entries may be constant or computed at runtime; the
compiler may optimize constant expressions. A `const STYLES: [Style; N]`
declaration requires compile-time evaluation. Naming reusable pieces as
`const` does not require every use or the style merge to execute at compile
time.

```rust
const ROW: Style = /* … */;
Aqua::Box().styles([ROW, if done { ROW_DONE } else { ROW_TODO }])
```

`Style::compose(&[a, b, c])` borrows the array as a slice; no allocation
unless a grid track is set. `Style` itself is `Clone` (not `Copy`:
grid tracks own a `Vec`); scalar-only styles never allocate.

## Builders: `styles: [...]`, no wrappers

Every rendered-element builder accepts `.styles([...])` (merging in
order, repeatable — a later `.styles([...])` call merges over the
earlier). The composed style applies in a single pass onto the retained
`LayoutStyle`/`Paint` pair: **no wrapper nodes**. `Button` carries paint
directly (background/radius/clip/border via `styles`), so the old
decorative `Box`-around-`Button` wrapper is gone.

Typography (`font_size` / `text_color` / `text_wrap`) resolves through
the same arrays with inheritance: a container's text color applies to
descendant labels unless a nearer style overrides it. The todo showcase
sets text styles explicitly per node.

## Lower-level compatibility

Prefer `Style` for authoring. `LayoutStyle`, `Paint`, `Border`,
`Shadow`, and `Length` remain exported (facade root) for theme adapters
and tree-level code; `Style::apply_to(&self, &mut layout, &mut paint)`
is the preferred tree-integration entry point. The bare `column`/`row`
axis constants stay out of the prelude (import `aqua::ui::column`
qualified) so lowercase consts never hijack user bindings.

## Table tracks: `fr` is not a `Length`

`aqua::fr(..)` builds a `ColumnWidth` for table columns only
(`Aqua::Column(..).width(fr(2.0))` vs `.width(px(90.0))`). It does not
extend `Length`: uniform `padding` / `margin` / `radius` stay px-only and
keep rejecting `Pct`/`Auto`. See [`AUTHORING.md`](AUTHORING.md).

## Current renderer limits

Unequal per-corner radii are represented by the resolved style but are
rejected with `LowerError::NonUniformCorners` when mounting or refreshing
an element. Uniform corners work; use `.radius(px(...))` for ordinary
rounded surfaces. Unsupported corners never silently become square or
uniform. Percentage padding, margins, and radii are also explicitly
rejected; percentage sizing is supported.

Generic node paint covers backgrounds, borders (including no background),
shadows, clipping, and whole-node opacity without wrapper nodes. Caret and
selection decorations also respect the input's and its ancestors' opacity.
