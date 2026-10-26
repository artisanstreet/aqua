# `Style` — canonical typed builder (`aqua-ui::style`)

Owner: style agent. Crate: `crates/aqua-ui/`. File: `src/style.rs`
(plus style-only tests in `tests/style_builder.rs`).
Status: **implemented** (this doc is normative for integrators).

## What it is

`Style` is the canonical public authoring API for styling. It is a
**partial** style: every field starts **unset**, and only explicitly set
fields affect resolution. `LayoutStyle` + `Paint` stay as the lower-level
resolved (total) compatibility types — the tree, layout (`to_taffy`), and
scene emission keep reading them unchanged.

```rust
use aqua_ui::style::{Style, auto, pct, px};

const S: Style = Style::new()
    .width(px(120.0))
    .height(pct(50.0))
    .padding(px(8.0))            // uniform px only; per-edge: padding_insets
    .radius(px(6.0))             // px only; per-corner: corners()
    .background(Oklch::new(0.6, 0.1, 250.0))
    .text_color(Oklch::BLACK)
    .font_size(14.0)
    .text_wrap(true);
```

All fields are **private**; construction is only via `Style::new()`
(`const fn`) and the consuming builders below. `Style` is
`Clone + Debug + Default + PartialEq` (not `Copy`: grid tracks own a
`Vec`; scalar-only styles never allocate).

## Length helpers (all `const fn`)

```rust
pub const fn px(v: f32) -> Length;   // Length::Px(v)
pub const fn pct(p: f32) -> Length;  // Length::Pct(p), 0..100+ of parent axis
pub const fn auto() -> Length;       // Length::Auto (content-sized / unbounded)
```

## Exact typed setters (all `const fn`; frozen)

Units never convert silently — there are **no** `From<Length>` (or
`From<f32>`-style) global conversions. Each setter takes one exact type:

```rust
// Sizing: full Length (px | pct | auto), no bare numbers.
pub const fn width(self, w: Length) -> Self;       // + height,
// min_width, min_height, max_width, max_height
//   .width(px(120.0)), .height(pct(50.0)), .height(auto())

// Spacing: uniform px ONLY — Pct/Auto panic instead of reinterpreting.
pub const fn padding(self, pad: Length) -> Self;   // px() only
pub const fn padding_insets(self, pad: Insets) -> Self; // per-edge
pub const fn margin(self, m: Length) -> Self;       // px() only
pub const fn margin_insets(self, m: Insets) -> Self;
//   .padding(px(8.0)), .padding_insets(Insets::hv(12.0, 8.0))

// Radius: px ONLY — Pct/Auto panic; per-corner via corners().
pub const fn radius(self, r: Length) -> Self;
//   .radius(px(6.0)), .corners(CornerRadii::uniform(4.0))
```

(`From<f32/f64/i32/u32/usize> for Length` in `element.rs` serve the
*element builders* only; `Style` takes exact `Length`. `From<f32>
for Insets` is impossible (orphan rule — both types foreign), so bare
`.padding(8.0)` is not a `Style` API.)

## Full setter list (covers every `LayoutStyle`/`Paint` property)

Layout — `width`, `height`, `min_width`, `min_height`, `max_width`,
`max_height` (exact `Length`), `flex_grow`, `flex_shrink`, `axis`,
`row()`, `column()`, `wrap` (flex wrap), `align`, `align_self` +
`clear_align_self`, `justify`, `gap`, `column_gap` +
`clear_column_gap`, `row_gap` + `clear_row_gap`, `padding` (px-only
`Length`) + `padding_insets(Insets)`, `margin` + `margin_insets`,
`grid_columns(Vec<Length>)` / `grid_columns_slice(&[Length])` +
`clear_grid_columns` (same for rows; `grid(&[Length], &[Length])`
sets both — grid setters are the only non-`const` builders),
`absolute(Absolute)` / `absolute_at(l, t, r, b)` +
`clear_absolute`.

Paint — `background(Oklch)` + `clear_background`, `radius`
(px-only `Length`), `corners(CornerRadii)` + `clear_corners`,
`border(Border)` / `border_of(w, color)` + `clear_border`,
`shadow(Shadow)` / `shadow_of(dx, dy, blur, color)` +
`clear_shadow`, `clip(bool)`, `opacity(f32)` + `clear_opacity`.

Typography (tree `TextContent` mirror: `font_size` 14.0,
`color` WHITE, `wrap` false) — `font_size(f32)`,
`text_color(Oklch)`, `text_wrap(bool)`. Note `wrap` is the flex
container flag; `text_wrap` is the text flag.

## Unset vs explicit-none (clearing)

Optional effects use two-level tracking: **unset** (inherits / leaves
the target alone) vs **explicit none** (actively clears the target).
Every `clear_*` setter records explicit-none:

```rust
Style::new().background(c).clear_background() // resolves fill -> None
Style::new().clear_border()                   // resolves border -> None
Style::new().clear_opacity()                  // resolves opacity -> None
Style::new().clear_align_self()               // resolves align_self -> None
Style::new().clear_grid_columns()             // resolves grid_columns -> None
```

Merging preserves the distinction: a later explicit-none overrides an
earlier value, and an unset field never overrides anything.

## Merge / compose (explicit ordered semantics)

Later entries override **only** explicitly set fields, in order:

```rust
impl Style {
    pub fn merge(&mut self, other: &Self);          // self absorbs other
    pub fn with(mut self, other: &Self) -> Self;    // builder form
    pub fn compose(styles: &[Style]) -> Style;      // fold in order
    pub const fn is_empty(&self) -> bool;
}
```

Fixed arrays work through the slice: `Style::compose(&[a, b, c])`
with `[Style; N]` — no allocation unless a grid track is set.

## Resolution (single pass, no wrapper nodes)

```rust
impl Style {
    /// Apply set fields onto an existing resolved pair (leaves the
    /// rest untouched). Preferred tree-integration entry point.
    pub fn apply_to(&self, layout: &mut LayoutStyle, paint: &mut Paint);

    /// Resolve against defaults (same as apply onto Default values).
    pub fn resolve_layout(&self) -> LayoutStyle;
    pub fn resolve_paint(&self) -> Paint;

    // Typography (defaults: 14.0 / Oklch::WHITE / false):
    pub fn font_size_opt(&self) -> Option<f32>;
    pub fn text_color_opt(&self) -> Option<Oklch>;
    pub fn text_wrap_opt(&self) -> Option<bool>;
    pub fn resolve_typography(&self) -> Typography;
    pub fn apply_typography(&self, font_size: &mut f32, color: &mut Oklch, wrap: &mut bool);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Typography { pub font_size: f32, pub color: Oklch, pub wrap: bool }
// impl Default for Typography  (14.0 / WHITE / false)
```

Tree-integration sketch (tree agent owns the call sites; style agent
guarantees these signatures):

```rust
let composed = Style::compose(&[theme, override_]);
let mut layout = LayoutStyle::default();
let mut paint = Paint::default();
composed.apply_to(&mut layout, &mut paint);
let typo = composed.resolve_typography();
// layout -> NodeSpec/style -> to_taffy; paint -> scene; typo -> TextContent
```

## `Oklch` const note (verified 2026-10-26, landed)

`Oklch::new(l, c, h)` is `const fn` (opaque) with const
`with_alpha(a)` — the const color path for styles:

```rust
const INK: Oklch = Oklch::new(0.2, 0.05, 250.0);
const FADED: Oklch = Oklch::new(0.6, 0.1, 250.0).with_alpha(0.5);
const PAPER: Oklch = Oklch::WHITE; // BLACK / WHITE / TRANSPARENT also const
```

`Oklch::oklch(l, c, h, a)` remains a plain (non-const) `pub fn`.
`Style::background` / `text_color` take `Oklch` by value (`Copy`),
so struct literals, constants, and `Oklch::new(..)` all work —
including inside `const` style expressions (locked by the
`full_const_chain_with_oklch_new` test).

## Performance

Scalar styling performs no allocation: every field except the two
grid-track lists is `Copy`-sized `Option`s. Grid setters (`Vec`)
are the only allocating builders and are documented as non-`const`.
