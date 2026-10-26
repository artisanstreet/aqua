//! Canonical `List<T>` / `Table<T>` / `Column<T>` builders and the frozen
//! collection source contract.
//!
//! This module is the normative home for collection authoring and sizing math.
//! The facade (`Aqua::List` / `Aqua::Table` / `Aqua::Column` / `fr`) forwards
//! to these builders without substitution; no second builder model exists.
//!
//! # Frozen runtime seam (single source: `collection_runtime`)
//!
//! [`CollectionLayout`], [`CollectionRole`], [`CollectionSource`],
//! [`CollectionDescriptor`], and [`CollectionError`] are defined once in
//! `crate::collection_runtime` (runtime owner) and re-exported here so both
//! owners share one definition — no duplicate seam, no conversion. The trait
//! signatures below ARE the frozen contract; the runtime peer consumes them
//! (realization, virtualization, `View::compute_layout`) and owns
//! `Element::collection`, `Element::role`, and all tree/binding work.
//!
//! # Builder contract
//!
//! - `items(impl IntoIterator<Item = T>)` snapshots row data (arrays, `Vec`s,
//!   and lazy iterators all work). Snapshots never re-read the source.
//! - `key(Fn(&T) -> K where K: Into<Key>)` declares the stable row key.
//!   Missing keys fail validation loudly, never fall back silently in valid
//!   flows. Row [`Element`]s are built WITHOUT keys: the runtime applies
//!   [`CollectionSource::key`] so reuse keys and anchor keys agree.
//! - `List::row(Fn(&T) -> Element)` and per-column cell factories are called
//!   only when the runtime requests a row (`source.row(..)`), never in bulk
//!   at `.build()` time. Unvirtualized mode mounts every row through those
//!   same factories; virtualized mode mounts a bounded window.
//! - `.virtualize()` is OFF by default (opt-in); `.estimated_row_height(..)`
//!   defaults to 32 logical px and `.overscan(..)` to 4 rows per side.
//! - `Table::columns(impl IntoIterator<Item = Column<T>>)` takes completed,
//!   uniform [`Column<T>`] values, so heterogeneous cell closures erase to one
//!   type and arrays work. Each column's OUTER cell box enforces the shared
//!   width; arbitrary inner header/cell [`Element`]s stay fully styleable.
//! - `header_styles` / `row_styles` / `cell_styles` are normal [`Style`]
//!   merges in call order. There is no special collection styling language.
//! - `fr(..)` is a table-track helper only (a `const fn` returning the plain
//!   [`ColumnWidth`] enum, so static tracks stay `const`); it never extends
//!   [`Length`]. Invalid weights are stored, never panicked on — the source
//!   rejects them at validation.
//! - Ordering/sorting stays application-owned (feed items pre-sorted).
//! - Fit-to-content is NOT offered: no method measures content, and no
//!   implicit scan of virtual rows ever happens. Explicit fixed `px` tracks
//!   (and `pct` of the table width) are the explicit sizing operations.
//!
//! # Authoritative column track placement
//!
//! Shared widths alone do not align columns: if the header row used a
//! different gap/padding than body rows, equal widths would still sit at
//! different x positions. So the track owns placement through a SHARED ROW
//! CHROME plan, not per-row styling:
//!
//! - Header, every body row, and horizontal scroll share the ONE width plan
//!   from [`CollectionSource::layout`] for the current available width, and
//!   the runtime mirrors one horizontal scroll offset across header and body.
//! - Row containers (header bar and body rows) MUST be plain row flex boxes:
//!   axis row, no wrap, justify start, align-self unset/stretch, no absolute
//!   positioning, no grid tracks. Anything else fails validation loudly
//!   (never silently discarded or coerced).
//! - Horizontal chrome — left/right padding, left/right margin, and the
//!   effective column gap — MUST MATCH between `header_styles` and
//!   `row_styles`. Only incompatible differences fail validation with a
//!   clear error; a shared nonzero gap/padding is normal styling and stays
//!   valid. The common inset `padL + padR + gap * (cols - 1)` is reserved up
//!   front: tracks allocate against `max(0, available - inset)` and
//!   `content_width = sum(column_widths) + inset`, so one scroll offset
//!   serves header and body.
//! - Vertical chrome — top/bottom padding and margins, row gap — is fully
//!   independent per part (header and body may differ): it never affects
//!   column x. Painted rounded row surfaces (fills, radii, borders) are
//!   paint, not layout, and are always free.
//! - Outer cell boxes (one per column): `flex_shrink(0)` cells at the shared
//!   widths, enforced after `cell_styles`. No left/right margin, no absolute
//!   positioning, no width/min/max or flex-grow overrides in `cell_styles`
//!   (rejected loudly instead of silently overridden). Cell PADDING (all
//!   sides) is allowed and lives INSIDE the assigned border-box column
//!   width; cell top/bottom margins are allowed. So padded cells keep the
//!   exact track width and x position.
//! - Inner header/cell content is arbitrary and fully styleable: different
//!   fonts, fills, buttons, or inner padding never move the column grid.
//!
//! # Roles and positions
//!
//! List rows wrap the arbitrary row element in an ordinary `Box` carrying
//! `Role::ListItem`, so inner controls keep their real roles (a Button row
//! stays a Button); table rows carry `Role::Row`, header cells carry
//! `Role::ColumnHeader`, and body cells carry `Role::Cell` (all via
//! `Element::role`). The table header row container carries `Role::Row`.
//! Logical positions and total counts (`CollectionSemantics`: table indices
//! zero-based, list positions one-based) are assigned by the runtime peer,
//! which owns `NodeSemantics.collection` propagation.

pub use crate::collection_runtime::{
    CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
};

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use aqua_core::Role;
use taffy::style::{AlignSelf, JustifyContent};

use crate::element::{self, Element, Key, NumberArg, Text};
use crate::style::{Axis, Length, Style};

type ElementFactory<T> = Box<dyn Fn(&T) -> Element>;
type KeyFactory<T> = Box<dyn Fn(&T) -> Key>;

// ---------------------------------------------------------------------------
// Table tracks: fixed px lengths or proportional fr shares (table-only)
// ---------------------------------------------------------------------------

/// Table column width: fixed [`Length`]s or proportional `fr` shares.
///
/// `fr` is a table-track helper only. It does not extend [`Length`]: padding,
/// margins, radii, and window sizes keep their px-only meaning. Plain data
/// (no validation inside): invalid tracks fail at source validation, and the
/// width plan sanitizes defensively — construction never panics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnWidth {
    /// Fixed width (`px(..)` exact, `pct(..)` of the table width).
    /// `Auto` is rejected at validation (it would require scanning rows).
    Fixed(Length),
    /// Proportional share of the remaining table width.
    Flex(f32),
}

impl From<Length> for ColumnWidth {
    fn from(v: Length) -> Self {
        ColumnWidth::Fixed(v)
    }
}

/// Proportional table-track share. Table-only; never a general [`Length`].
///
/// `const`-friendly so static tracks stay `const` alongside a `const`
/// [`Style`]: `const TRACK: ColumnWidth = fr(2.0);`. `fr(1.0)` / `fr(2.0)`
/// split the width left after fixed tracks 1:2. Invalid weights (negative,
/// non-finite) are stored as-is and rejected by source validation — this
/// helper never panics.
#[must_use]
pub const fn fr(weight: f32) -> ColumnWidth {
    ColumnWidth::Flex(weight)
}

// ---------------------------------------------------------------------------
// Column: completed, uniform, closure-erased
// ---------------------------------------------------------------------------

/// Completed table column: id, header factory, type-erased cell factory, and
/// track sizing. Heterogeneous cells in arrays work because closures erase to
/// `Box<dyn Fn>` at completion ([`ColumnBuilder::build`]).
pub struct Column<T> {
    id: String,
    header: Box<dyn Fn() -> Element>,
    cell: Option<ElementFactory<T>>,
    width: ColumnWidth,
    min_width: Option<f32>,
    max_width: Option<f32>,
}

impl<T> Column<T> {
    /// Column id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Column builder: id first, then arbitrary header/cell/width.
///
/// The header is a factory (`Fn() -> Element`) so the runtime can rebuild the
/// header row per width plan without consuming it; call it with a closure
/// producing any [`Element`] (text, buttons, …). Cells are per-row factories
/// over the snapshot item.
pub struct ColumnBuilder<T> {
    id: String,
    header: Option<Box<dyn Fn() -> Element>>,
    cell: Option<ElementFactory<T>>,
    width: ColumnWidth,
    min_width: Option<f32>,
    max_width: Option<f32>,
}

impl<T> ColumnBuilder<T> {
    /// Start a column builder with its id.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            header: None,
            cell: None,
            width: ColumnWidth::Flex(1.0),
            min_width: None,
            max_width: None,
        }
    }

    /// Arbitrary header [`Element`] factory (real text/buttons, never fake
    /// controls). Called by the runtime per header realization.
    pub fn header(mut self, header: impl Fn() -> Element + 'static) -> Self {
        self.header = Some(Box::new(header));
        self
    }

    /// Cell factory per row item. Called only for mounted rows, never in bulk
    /// at build time.
    pub fn cell(mut self, f: impl Fn(&T) -> Element + 'static) -> Self {
        self.cell = Some(Box::new(f));
        self
    }

    /// Track width: `px(..)` lengths, `pct(..)` of the table width, or
    /// `fr(..)` shares of the remainder. Accepts any
    /// `impl Into<ColumnWidth>` (so `px(..)`/`pct(..)`/`auto()` lengths and
    /// `fr(..)` shares all work); `Auto` tracks fail validation loudly.
    pub fn width(mut self, width: impl Into<ColumnWidth>) -> Self {
        self.width = width.into();
        self
    }

    /// Minimum cell width in logical px (enforced after share computation;
    /// totals may then exceed the available width and scroll).
    pub fn min_width(mut self, min: impl NumberArg) -> Self {
        self.min_width = Some(min.into_f32());
        self
    }

    /// Maximum cell width in logical px.
    pub fn max_width(mut self, max: impl NumberArg) -> Self {
        self.max_width = Some(max.into_f32());
        self
    }

    /// Complete the column, erasing the closure types. Missing factories are
    /// NOT substituted here: a missing cell fails table validation loudly.
    pub fn build(self) -> Column<T> {
        Column {
            id: self.id,
            header: self
                .header
                .unwrap_or_else(|| Box::new(|| Text::builder().arg("").build())),
            cell: self.cell,
            width: self.width,
            min_width: self.min_width,
            max_width: self.max_width,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared validation + width-plan math (no graph, no layout engine)
// ---------------------------------------------------------------------------

/// Collect stable keys or fail on a missing/duplicate key factory. Visiting
/// every *key* once is bounded and required; row factories are never called.
fn collect_keys<T>(
    items: &[T],
    key_fn: Option<&dyn Fn(&T) -> Key>,
) -> Result<Vec<Key>, CollectionError> {
    let Some(key_fn) = key_fn else {
        return Err(CollectionError::MissingKeyFactory);
    };
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let key = key_fn(item);
        if !seen.insert(key.clone()) {
            return Err(CollectionError::DuplicateKey);
        }
        out.push(key);
    }
    Ok(out)
}

/// Resolve the estimated row height or fail loudly (finite positive `px`).
fn check_row_height(height: Length) -> Result<f32, CollectionError> {
    match height {
        Length::Px(v) if v.is_finite() && v > 0.0 => Ok(v),
        _ => Err(CollectionError::InvalidSize(format!(
            "estimated row height {height:?}: expected finite positive px(..)"
        ))),
    }
}

/// Raw height carrier for descriptor construction (infallible): `Px(v)` passes
/// the original value through untouched — even when invalid — so explicit
/// validation fails loudly instead of being silently defaulted. Non-`px`
/// lengths have no `f32` meaning and carrier `NaN`, which validation also
/// rejects. The runtime's own estimate sanitizer stays as its last-resort
/// backstop, never as the loud gate.
fn height_f32(height: Length) -> f32 {
    match height {
        Length::Px(v) => v,
        _ => f32::NAN,
    }
}

/// Validate one column's track and bounds.
fn check_column<T>(col: &Column<T>) -> Result<(), CollectionError> {
    if col.id.is_empty() {
        return Err(CollectionError::Validation(
            "table column id must not be empty".into(),
        ));
    }
    if col.cell.is_none() {
        return Err(CollectionError::Validation(format!(
            "column `{}` is missing .cell(..): a cell factory is required",
            col.id
        )));
    }
    let track_ok = match col.width {
        ColumnWidth::Fixed(Length::Px(v) | Length::Pct(v)) => v.is_finite() && v >= 0.0,
        ColumnWidth::Fixed(Length::Auto) => false,
        ColumnWidth::Flex(w) => w.is_finite() && w >= 0.0,
    };
    if !track_ok {
        return Err(CollectionError::InvalidTrack(format!(
            "column `{}` has an invalid track width: expected finite non-negative \
             px(..), pct(..), or fr(..) (Auto would require scanning virtual rows)",
            col.id
        )));
    }
    let bounds_ok = col.min_width.is_none_or(|m| m.is_finite() && m >= 0.0)
        && col.max_width.is_none_or(|m| m.is_finite() && m >= 0.0)
        && match (col.min_width, col.max_width) {
            (Some(min), Some(max)) => min <= max,
            _ => true,
        };
    if !bounds_ok {
        return Err(CollectionError::InvalidSize(format!(
            "column `{}` has invalid width bounds (min: {:?}, max: {:?}): \
             expected finite non-negative bounds with min <= max",
            col.id, col.min_width, col.max_width
        )));
    }
    Ok(())
}

/// Shared horizontal chrome of one row container: left/right padding,
/// left/right margin, and the effective column gap. Header bar and body rows
/// must agree exactly; only incompatible differences fail validation.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RowChrome {
    pad_l: f32,
    pad_r: f32,
    mar_l: f32,
    mar_r: f32,
    gap: f32,
}

/// Resolve and constrain one row container's styles: plain row flex model
/// (axis row, no wrap, justify start, unset/stretch self, in-flow, no grid),
/// finite chrome, non-negative padding/gaps. Vertical chrome (top/bottom
/// padding and margins, row gap) is free beyond finiteness; paint and
/// typography are untouched by this check.
fn row_chrome(what: &'static str, styles: &[Style]) -> Result<RowChrome, CollectionError> {
    let resolved = Style::compose(styles).resolve_layout();
    let bad = |why: String| {
        CollectionError::Validation(format!(
            "table {what} styles break the column track: {why} (row containers \
             must be axis row, no wrap, justify start, in-flow, grid-free; \
             horizontal padding/margins/gap must match across header and body)"
        ))
    };
    if resolved.axis != Axis::Row {
        return Err(bad("axis must be row".into()));
    }
    if resolved.wrap {
        return Err(bad("wrap must be off".into()));
    }
    if resolved.justify != JustifyContent::START {
        return Err(bad("justify must be start".into()));
    }
    if !matches!(resolved.align_self, None | Some(AlignSelf::STRETCH)) {
        return Err(bad("align_self must be unset or stretch".into()));
    }
    if resolved.absolute.is_some() {
        return Err(bad("absolute positioning is unsupported on rows".into()));
    }
    if resolved.grid_columns.is_some() || resolved.grid_rows.is_some() {
        return Err(bad("grid tracks are unsupported on rows".into()));
    }
    let gap = resolved.column_gap.unwrap_or(resolved.gap);
    for (name, v) in [
        ("gap", gap),
        ("padding.left", resolved.padding.left),
        ("padding.right", resolved.padding.right),
        ("padding.top", resolved.padding.top),
        ("padding.bottom", resolved.padding.bottom),
        ("margin.left", resolved.margin.left),
        ("margin.right", resolved.margin.right),
        ("margin.top", resolved.margin.top),
        ("margin.bottom", resolved.margin.bottom),
    ] {
        if !v.is_finite() {
            return Err(bad(format!("{name} must be finite")));
        }
    }
    if gap < 0.0 {
        return Err(bad("gap must be >= 0".into()));
    }
    if resolved.padding.left < 0.0
        || resolved.padding.right < 0.0
        || resolved.padding.top < 0.0
        || resolved.padding.bottom < 0.0
    {
        return Err(bad("padding must be >= 0".into()));
    }
    Ok(RowChrome {
        pad_l: resolved.padding.left,
        pad_r: resolved.padding.right,
        mar_l: resolved.margin.left,
        mar_r: resolved.margin.right,
        gap,
    })
}

/// The shared plan requires identical horizontal chrome: any difference in
/// left/right padding, left/right margin, or column gap would put header and
/// body columns at different x positions. Vertical chrome is intentionally
/// NOT compared (header and body may differ freely).
fn check_row_chrome_match(header: &RowChrome, body: &RowChrome) -> Result<(), CollectionError> {
    let mismatch = |what: &str, h: f32, b: f32| {
        CollectionError::Validation(format!(
            "table header_styles and row_styles disagree on {what} ({h} vs {b}): \
             horizontal padding L/R, margins L/R, and column gap must match so \
             header and body columns share x positions (vertical \
             padding/margins may differ freely)"
        ))
    };
    if header.pad_l != body.pad_l {
        return Err(mismatch("left padding", header.pad_l, body.pad_l));
    }
    if header.pad_r != body.pad_r {
        return Err(mismatch("right padding", header.pad_r, body.pad_r));
    }
    if header.mar_l != body.mar_l {
        return Err(mismatch("left margin", header.mar_l, body.mar_l));
    }
    if header.mar_r != body.mar_r {
        return Err(mismatch("right margin", header.mar_r, body.mar_r));
    }
    if header.gap != body.gap {
        return Err(mismatch("column gap", header.gap, body.gap));
    }
    Ok(())
}

/// Sanitized shared chrome for layout math (infallible): row styles win when
/// present, else header styles. Returns `(pad_l, pad_r, gap)` with insane
/// inputs sanitized to zero; validation is the loud gate, this is the
/// defensive fallback.
fn table_chrome(row_styles: &[Style], header_styles: &[Style]) -> (f32, f32, f32) {
    let src = if row_styles.is_empty() {
        header_styles
    } else {
        row_styles
    };
    let resolved = Style::compose(src).resolve_layout();
    let sane = |v: f32| {
        if v.is_finite() && v >= 0.0 {
            v
        } else {
            0.0
        }
    };
    (
        sane(resolved.padding.left),
        sane(resolved.padding.right),
        sane(resolved.column_gap.unwrap_or(resolved.gap)),
    )
}

/// Common inset reserved up front: `padL + padR + gap * (cols - 1)`.
fn chrome_inset(pad_l: f32, pad_r: f32, gap: f32, cols: usize) -> f32 {
    pad_l + pad_r + gap * cols.saturating_sub(1) as f32
}

/// Outer cell boxes are track-placed: left/right margins or out-of-flow
/// positioning would shift columns off the plan (rejected loudly);
/// width/min/max and flex-grow must come from the column track alone
/// (rejected loudly instead of silently overridden). `flex_shrink(0)` is
/// enforced after styles so narrow viewports overflow (scroll) instead of
/// squeezing tracks. Padding (all sides) is allowed: it lives INSIDE the
/// assigned border-box column width. Top/bottom cell margins are allowed.
fn check_cell_styles(styles: &[Style]) -> Result<(), CollectionError> {
    let resolved = Style::compose(styles).resolve_layout();
    if resolved.margin.left != 0.0 || resolved.margin.right != 0.0 {
        return Err(CollectionError::Validation(
            "table cell_styles must not set left/right margin: margins sit \
             outside the fixed tracks and would shift column x positions; put \
             spacing as padding inside cells (border-box)"
                .into(),
        ));
    }
    if resolved.absolute.is_some() {
        return Err(CollectionError::Validation(
            "table cell_styles must not use absolute positioning: cells are \
             track-placed"
                .into(),
        ));
    }
    if resolved.width != Length::Auto
        || resolved.min_width != Length::Auto
        || resolved.max_width != Length::Auto
        || resolved.flex_grow != 0.0
    {
        return Err(CollectionError::InvalidSize(
            "table cell_styles must not set width/min_width/max_width/flex_grow: \
             the shared column track (plus column min/max) is enforced on \
             every outer cell box"
                .into(),
        ));
    }
    Ok(())
}

/// Validate every table structural rule that is independent of row data:
/// columns present, ids non-empty and unique, per-column factories/tracks/
/// bounds (in column order), then row-container and cell style constraints.
fn validate_table_structure<T>(
    columns: &[Column<T>],
    header_styles: &[Style],
    row_styles: &[Style],
    cell_styles: &[Style],
) -> Result<(), CollectionError> {
    if columns.is_empty() {
        return Err(CollectionError::Validation(
            "table has no columns: at least one column is required".into(),
        ));
    }
    let mut seen = HashSet::new();
    for col in columns {
        if col.id.is_empty() {
            return Err(CollectionError::Validation(
                "table column id must not be empty".into(),
            ));
        }
        if !seen.insert(col.id.clone()) {
            return Err(CollectionError::DuplicateColumnId);
        }
    }
    for col in columns {
        check_column(col)?;
    }
    let header_chrome = row_chrome("header_styles", header_styles)?;
    let body_chrome = row_chrome("row_styles", row_styles)?;
    check_row_chrome_match(&header_chrome, &body_chrome)?;
    check_cell_styles(cell_styles)?;
    Ok(())
}

/// Shared width plan: fixed tracks exact, `pct` of the available width, `fr`
/// shares split the remainder, then per-column min/max clamp. Pure math over
/// the track list — visits no cells, builds no elements. Sanitizes insane
/// inputs to zero instead of panicking; totals may exceed the available width
/// when minimums force it (the table then scrolls horizontally).
/// Shared width plan: the common inset (`padL + padR + gap * (cols - 1)`) is
/// reserved up front and tracks allocate against `max(0, available - inset)`.
/// Fixed tracks exact, `pct` of the track area, `fr` shares split the
/// remainder, then per-column min/max clamp. Pure math over the track list —
/// visits no cells, builds no elements. Sanitizes insane inputs to zero
/// instead of panicking; totals may exceed the track area when minimums force
/// it (the table then scrolls horizontally).
fn plan_widths(
    tracks: &[(ColumnWidth, Option<f32>, Option<f32>)],
    available_width: f32,
    inset: f32,
) -> Vec<f32> {
    let available = if available_width.is_finite() {
        available_width.max(0.0)
    } else {
        0.0
    };
    let inset = if inset.is_finite() && inset > 0.0 {
        inset
    } else {
        0.0
    };
    let available = (available - inset).max(0.0);
    let fixed_total: f32 = tracks
        .iter()
        .map(|(track, _, _)| match track {
            ColumnWidth::Fixed(Length::Px(v)) if v.is_finite() && *v >= 0.0 => *v,
            ColumnWidth::Fixed(Length::Pct(p)) if p.is_finite() && *p >= 0.0 => {
                available * p / 100.0
            }
            _ => 0.0,
        })
        .sum();
    let flex_total: f32 = tracks
        .iter()
        .map(|(track, _, _)| match track {
            ColumnWidth::Flex(w) if w.is_finite() && *w >= 0.0 => *w,
            _ => 0.0,
        })
        .sum();
    let remaining = (available - fixed_total).max(0.0);
    tracks
        .iter()
        .map(|(track, min, max)| {
            let base = match track {
                ColumnWidth::Fixed(Length::Px(v)) if v.is_finite() && *v >= 0.0 => *v,
                ColumnWidth::Fixed(Length::Pct(p)) if p.is_finite() && *p >= 0.0 => {
                    available * p / 100.0
                }
                ColumnWidth::Fixed(_) => 0.0,
                ColumnWidth::Flex(w) if w.is_finite() && *w >= 0.0 && flex_total > 0.0 => {
                    remaining * *w / flex_total
                }
                ColumnWidth::Flex(_) => 0.0,
            };
            let lo = min.unwrap_or(0.0);
            let hi = max.unwrap_or(f32::INFINITY);
            base.clamp(lo.max(0.0), hi.max(lo.max(0.0)))
        })
        .collect()
}

/// Outer cell box: merges `cell_styles` first, then enforces the shared column
/// width plus column min/max AND `flex_shrink(0)` AFTER styles (documented
/// precedence: the track always wins, even if validation was skipped, so
/// narrow viewports overflow into horizontal scroll instead of squeezing
/// tracks). Wraps the arbitrary inner element untouched.
fn outer_cell(
    width: f32,
    min: Option<f32>,
    max: Option<f32>,
    cell_styles: &[Style],
) -> element::Box {
    let mut cell = element::Box::new();
    for s in cell_styles {
        cell = cell.style(s.clone());
    }
    cell = cell.width(Length::Px(width));
    cell = cell.style(Style::new().flex_shrink(0.0));
    if let Some(min) = min {
        cell = cell.style(Style::new().min_width(Length::Px(min)));
    }
    if let Some(max) = max {
        cell = cell.style(Style::new().max_width(Length::Px(max)));
    }
    cell
}

// ---------------------------------------------------------------------------
// List<T>: builder + immutable snapshot source
// ---------------------------------------------------------------------------

/// List builder: snapshot items, stable keys, one row [`Element`] per item.
///
/// `styles` merge onto the collection container in order. Virtualization is
/// OFF by default (`.virtualize()` opts in). `.build()` lowers via the
/// runtime-owned `Element::collection`.
pub struct List<T> {
    items: Vec<T>,
    key_fn: Option<KeyFactory<T>>,
    row_fn: Option<ElementFactory<T>>,
    styles: Vec<Style>,
    virtualized: bool,
    estimated_row_height: Length,
    overscan: usize,
}

impl<T> List<T> {
    /// Start an empty list builder.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            key_fn: None,
            row_fn: None,
            styles: Vec::new(),
            virtualized: false,
            estimated_row_height: Length::Px(32.0),
            overscan: 4,
        }
    }

    /// Row data (any `IntoIterator`; arrays, `Vec`s, and lazy iterators work).
    /// Snapshotted at call time; later source mutations are not observed.
    pub fn items(mut self, items: impl IntoIterator<Item = T>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    /// Stable per-row key (`K: Into<Key>`). Required: missing keys fail
    /// [`List::validate`] / [`CollectionSource::validate`].
    pub fn key<K>(mut self, f: impl Fn(&T) -> K + 'static) -> Self
    where
        K: Into<Key>,
    {
        self.key_fn = Some(Box::new(move |v| f(v).into()));
        self
    }

    /// Row factory: arbitrary [`Element`] per item. Called only for mounted
    /// rows (never in bulk at build time). The runtime applies the stable key;
    /// the source applies `Role::ListItem`.
    pub fn row(mut self, f: impl Fn(&T) -> Element + 'static) -> Self {
        self.row_fn = Some(Box::new(f));
        self
    }

    /// Apply one container style (call order with every other setter).
    pub fn style(mut self, style: Style) -> Self {
        self.styles.push(style);
        self
    }

    /// Apply several container styles in order.
    pub fn styles(mut self, styles: impl AsRef<[Style]>) -> Self {
        self.styles.extend(styles.as_ref().iter().cloned());
        self
    }

    /// Opt in to virtualization (OFF by default). The runtime then realizes a
    /// bounded window; estimated config below seeds its estimates.
    pub fn virtualize(mut self) -> Self {
        self.virtualized = true;
        self
    }

    /// Seed row height for virtualized estimates: `px(..)` only (default
    /// `px(32)`). The original length is preserved end to end: anything but
    /// finite positive `px` fails validation loudly instead of being silently
    /// defaulted.
    pub fn estimated_row_height(mut self, height: Length) -> Self {
        self.estimated_row_height = height;
        self
    }

    /// Mounted rows kept past each visible edge (default 4).
    pub fn overscan(mut self, overscan: usize) -> Self {
        self.overscan = overscan;
        self
    }

    /// Whether `.virtualize()` was selected.
    #[must_use]
    pub fn is_virtualized(&self) -> bool {
        self.virtualized
    }

    /// Row count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the list holds no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Fail loudly on missing factories/keys, duplicate keys, and invalid
    /// estimated heights. The runtime runs this (via the source plus its own
    /// `validate_snapshot`) at mount/layout.
    pub fn validate(&self) -> Result<(), CollectionError> {
        collect_keys(&self.items, self.key_fn.as_deref())?;
        if self.row_fn.is_none() {
            return Err(CollectionError::MissingRowFactory);
        }
        check_row_height(self.estimated_row_height)?;
        Ok(())
    }

    /// Lower to the runtime-owned collection element.
    pub fn build(self) -> Element
    where
        T: 'static,
    {
        Element::collection(self.descriptor())
    }

    /// Snapshot this builder into its validated-later descriptor. Factories
    /// are NOT called here; the runtime calls them per mounted row.
    pub fn descriptor(self) -> CollectionDescriptor
    where
        T: 'static,
    {
        let source: Rc<dyn CollectionSource> = Rc::new(ListSource {
            items: self.items,
            key_fn: self.key_fn,
            row_fn: self.row_fn,
            estimated_row_height: self.estimated_row_height,
        });
        CollectionDescriptor::new(
            source,
            self.styles,
            self.virtualized,
            height_f32(self.estimated_row_height),
            self.overscan,
        )
    }
}

impl<T> Default for List<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable list snapshot source (owned by [`CollectionDescriptor`]).
pub struct ListSource<T> {
    items: Vec<T>,
    key_fn: Option<KeyFactory<T>>,
    row_fn: Option<ElementFactory<T>>,
    estimated_row_height: Length,
}

impl<T> CollectionSource for ListSource<T> {
    fn len(&self) -> usize {
        self.items.len()
    }

    fn key(&self, index: usize) -> Key {
        match &self.key_fn {
            Some(f) => f(&self.items[index]),
            // Unreachable post-validation (MissingKeyFactory); positional
            // fallback keeps this infallible instead of panicking.
            None => Key::Usize(index),
        }
    }

    fn layout(&self, available_width: f32) -> CollectionLayout {
        let content_width = if available_width.is_finite() {
            available_width.max(0.0)
        } else {
            0.0
        };
        CollectionLayout {
            content_width,
            column_widths: Vec::new(),
        }
    }

    fn row(&self, index: usize, _layout: &CollectionLayout) -> Element {
        // NOTE: no key is set here on purpose — the runtime applies
        // `CollectionSource::key` to the wrapper so reuse keys and anchor
        // keys agree.
        // Structural collection wrapper: the arbitrary row element keeps its
        // own role/behavior (a Button row stays a Button, an editable field
        // stays a TextField), while the ordinary outer Box carries
        // `Role::ListItem` for collection geometry/identity. Binding focus
        // and keyed reuse track the wrapper. One extra node per resident row
        // is structural semantics, not a decorative wrapper.
        let body = match (&self.row_fn, self.items.get(index)) {
            (Some(f), Some(item)) => f(item),
            // Unreachable post-validation (missing factory): loud visible
            // marker, never a silent omission or a panic.
            _ => Text::builder().arg("[aqua] List::row(..) missing").build(),
        };
        element::Box::new().child(body).build().role(Role::ListItem)
    }

    fn header(&self, _layout: &CollectionLayout) -> Option<Element> {
        None
    }

    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }

    fn validate(&self) -> Result<(), CollectionError> {
        collect_keys(&self.items, self.key_fn.as_deref())?;
        if self.row_fn.is_none() {
            return Err(CollectionError::MissingRowFactory);
        }
        check_row_height(self.estimated_row_height)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Table<T>: builder + immutable snapshot source
// ---------------------------------------------------------------------------

/// Table builder: snapshot items plus completed [`Column<T>`]s.
///
/// `styles` merge onto the collection container in order (viewport bounds
/// live here — e.g. `.styles([TABLE_BOX])`); `header_styles` / `row_styles`
/// / `cell_styles` are normal [`Style`] merges subject to the shared-chrome
/// constraints documented at the top of this module (horizontal row chrome
/// must match across header/body; vertical chrome is independent; cells: no
/// horizontal margin, no width/flex-grow overrides; cell padding is
/// border-box and free).
/// Header, every body row, and horizontal scroll share one width plan.
/// Virtualization is OFF by default. `.build()` lowers via the runtime-owned
/// `Element::collection`.
pub struct Table<T> {
    items: Vec<T>,
    columns: Vec<Column<T>>,
    key_fn: Option<KeyFactory<T>>,
    styles: Vec<Style>,
    header_styles: Vec<Style>,
    row_styles: Vec<Style>,
    cell_styles: Vec<Style>,
    virtualized: bool,
    estimated_row_height: Length,
    overscan: usize,
}

impl<T> Table<T> {
    /// Start an empty table builder.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            columns: Vec::new(),
            key_fn: None,
            styles: Vec::new(),
            header_styles: Vec::new(),
            row_styles: Vec::new(),
            cell_styles: Vec::new(),
            virtualized: false,
            estimated_row_height: Length::Px(32.0),
            overscan: 4,
        }
    }

    /// Row data (any `IntoIterator`). Snapshotted at call time.
    pub fn items(mut self, items: impl IntoIterator<Item = T>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    /// Stable per-row key (`K: Into<Key>`). Required. The runtime applies it;
    /// sources must not set it (so reuse and anchor keys agree).
    pub fn key<K>(mut self, f: impl Fn(&T) -> K + 'static) -> Self
    where
        K: Into<Key>,
    {
        self.key_fn = Some(Box::new(move |v| f(v).into()));
        self
    }

    /// Completed columns (arrays, `Vec`s, and iterators work; heterogeneous
    /// cell closures erase to the uniform [`Column<T>`] at completion).
    pub fn columns(mut self, columns: impl IntoIterator<Item = Column<T>>) -> Self {
        self.columns = columns.into_iter().collect();
        self
    }

    /// Apply one container style (call order with every other setter).
    /// These merge onto the collection container (viewport bounds live here);
    /// header/row/cell parts keep their own style lists below.
    pub fn style(mut self, style: Style) -> Self {
        self.styles.push(style);
        self
    }

    /// Apply several container styles in order (e.g. `.styles([TABLE_BOX])`
    /// to bound the viewport the table virtualizes inside).
    pub fn styles(mut self, styles: impl AsRef<[Style]>) -> Self {
        self.styles.extend(styles.as_ref().iter().cloned());
        self
    }

    /// Header-row styles (normal merges, call order; track-placement
    /// constraints apply — see module docs).
    pub fn header_styles(mut self, styles: impl AsRef<[Style]>) -> Self {
        self.header_styles.extend(styles.as_ref().iter().cloned());
        self
    }

    /// Body-row styles (normal merges, call order; track-placement
    /// constraints apply — see module docs).
    pub fn row_styles(mut self, styles: impl AsRef<[Style]>) -> Self {
        self.row_styles.extend(styles.as_ref().iter().cloned());
        self
    }

    /// Per-cell outer-box styles (normal merges, call order). Inner cell
    /// elements stay fully styleable by their own factories; cell padding is
    /// border-box inside the track width.
    pub fn cell_styles(mut self, styles: impl AsRef<[Style]>) -> Self {
        self.cell_styles.extend(styles.as_ref().iter().cloned());
        self
    }

    /// Opt in to virtualization (OFF by default).
    pub fn virtualize(mut self) -> Self {
        self.virtualized = true;
        self
    }

    /// Seed row height for virtualized estimates: `px(..)` only (default
    /// `px(32)`).
    pub fn estimated_row_height(mut self, height: Length) -> Self {
        self.estimated_row_height = height;
        self
    }

    /// Mounted rows kept past each visible edge (default 4).
    pub fn overscan(mut self, overscan: usize) -> Self {
        self.overscan = overscan;
        self
    }

    /// Whether `.virtualize()` was selected.
    #[must_use]
    pub fn is_virtualized(&self) -> bool {
        self.virtualized
    }

    /// Row count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the table holds no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Column count.
    #[must_use]
    pub fn column_len(&self) -> usize {
        self.columns.len()
    }

    /// Shared width plan for `available_width` (same math the source uses for
    /// header, rows, and horizontal scroll): the common inset
    /// (`padL + padR + gap * (cols - 1)`) is reserved up front and tracks
    /// allocate against the remainder. Pure sizing: visits no cells.
    #[must_use]
    pub fn column_widths(&self, available_width: f32) -> Vec<f32> {
        let (pad_l, pad_r, gap) = table_chrome(&self.row_styles, &self.header_styles);
        let inset = chrome_inset(pad_l, pad_r, gap, self.columns.len());
        plan_widths(&self.track_specs(), available_width, inset)
    }

    /// Fail loudly on missing columns/factories/keys, duplicate keys/column
    /// ids, invalid sizes/tracks/bounds, and row/cell styles that would break
    /// track placement. Deterministic check order: columns present, column
    /// ids non-empty and unique, per-column factories then tracks then bounds
    /// (in column order), row-container and cell style constraints, row keys
    /// then duplicates, estimated height last.
    pub fn validate(&self) -> Result<(), CollectionError> {
        validate_table_structure(
            &self.columns,
            &self.header_styles,
            &self.row_styles,
            &self.cell_styles,
        )?;
        collect_keys(&self.items, self.key_fn.as_deref())?;
        check_row_height(self.estimated_row_height)?;
        Ok(())
    }

    /// Lower to the runtime-owned collection element.
    pub fn build(self) -> Element
    where
        T: 'static,
    {
        Element::collection(self.descriptor())
    }

    /// Snapshot this builder into its validated-later descriptor. Factories
    /// are NOT called here.
    pub fn descriptor(self) -> CollectionDescriptor
    where
        T: 'static,
    {
        let source: Rc<dyn CollectionSource> = Rc::new(TableSource {
            items: self.items,
            columns: self.columns,
            key_fn: self.key_fn,
            header_styles: self.header_styles,
            row_styles: self.row_styles,
            cell_styles: self.cell_styles,
            estimated_row_height: self.estimated_row_height,
            plan_cache: RefCell::new(None),
        });
        CollectionDescriptor::new(
            source,
            self.styles,
            self.virtualized,
            height_f32(self.estimated_row_height),
            self.overscan,
        )
    }

    fn track_specs(&self) -> Vec<(ColumnWidth, Option<f32>, Option<f32>)> {
        self.columns
            .iter()
            .map(|c| (c.width, c.min_width, c.max_width))
            .collect()
    }
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable table snapshot source (owned by [`CollectionDescriptor`]).
///
/// Caches one width plan per available width (keyed by `f32` bits); the plan
/// is pure track math and never visits cells.
pub struct TableSource<T> {
    items: Vec<T>,
    columns: Vec<Column<T>>,
    key_fn: Option<KeyFactory<T>>,
    header_styles: Vec<Style>,
    row_styles: Vec<Style>,
    cell_styles: Vec<Style>,
    estimated_row_height: Length,
    plan_cache: RefCell<Option<(u32, Vec<f32>)>>,
}

impl<T> TableSource<T> {
    fn track_specs(&self) -> Vec<(ColumnWidth, Option<f32>, Option<f32>)> {
        self.columns
            .iter()
            .map(|c| (c.width, c.min_width, c.max_width))
            .collect()
    }

    fn plan(&self, available_width: f32) -> Vec<f32> {
        let available = if available_width.is_finite() {
            available_width.max(0.0)
        } else {
            0.0
        };
        let bits = available.to_bits();
        if let Some((cached, widths)) = self.plan_cache.borrow().as_ref() {
            if *cached == bits {
                return widths.clone();
            }
        }
        let (pad_l, pad_r, gap) = table_chrome(&self.row_styles, &self.header_styles);
        let inset = chrome_inset(pad_l, pad_r, gap, self.columns.len());
        let widths = plan_widths(&self.track_specs(), available, inset);
        *self.plan_cache.borrow_mut() = Some((bits, widths.clone()));
        widths
    }

    /// Common inset for the current style lists (see [`table_chrome`]).
    fn inset(&self) -> f32 {
        let (pad_l, pad_r, gap) = table_chrome(&self.row_styles, &self.header_styles);
        chrome_inset(pad_l, pad_r, gap, self.columns.len())
    }

    fn cell_box(&self, col_index: usize, width: f32, inner: Element) -> Element {
        let col = &self.columns[col_index];
        outer_cell(width, col.min_width, col.max_width, &self.cell_styles)
            .child(inner)
            .build()
    }
}

impl<T> CollectionSource for TableSource<T> {
    fn len(&self) -> usize {
        self.items.len()
    }

    fn key(&self, index: usize) -> Key {
        match &self.key_fn {
            Some(f) => f(&self.items[index]),
            // Unreachable post-validation (MissingKeyFactory); positional
            // fallback keeps this infallible instead of panicking.
            None => Key::Usize(index),
        }
    }

    fn layout(&self, available_width: f32) -> CollectionLayout {
        let column_widths = self.plan(available_width);
        CollectionLayout {
            content_width: column_widths.iter().sum::<f32>() + self.inset(),
            column_widths,
        }
    }

    fn row(&self, index: usize, layout: &CollectionLayout) -> Element {
        // NOTE: no key is set here on purpose — the runtime applies
        // `CollectionSource::key` so reuse keys and anchor keys agree.
        // Row containers apply the author's original styles: header and body
        // share the validated horizontal chrome, so every body row places
        // column `i` at exactly the shared plan offset.
        let mut row = element::Box::new();
        for s in &self.row_styles {
            row = row.style(s.clone());
        }
        for (i, col) in self.columns.iter().enumerate() {
            let width = layout.column_widths.get(i).copied().unwrap_or(0.0);
            let inner = match (&col.cell, self.items.get(index)) {
                (Some(f), Some(item)) => f(item),
                // Unreachable post-validation (missing factory): loud
                // visible marker, never a silent omission or a panic.
                _ => Text::builder()
                    .arg("[aqua] Column::cell(..) missing")
                    .build(),
            };
            let cell = self.cell_box(i, width, inner).role(Role::Cell);
            row = row.child(cell);
        }
        row.build().role(Role::Row)
    }

    fn header(&self, layout: &CollectionLayout) -> Option<Element> {
        // Same placement contract as `row`: plain row container, same plan,
        // same outer widths — so header column `i` sits at exactly the body
        // column `i` x position regardless of header styling.
        let mut bar = element::Box::new();
        for s in &self.header_styles {
            bar = bar.style(s.clone());
        }
        for (i, col) in self.columns.iter().enumerate() {
            let width = layout.column_widths.get(i).copied().unwrap_or(0.0);
            let cell = self
                .cell_box(i, width, (col.header)())
                .role(Role::ColumnHeader);
            bar = bar.child(cell);
        }
        Some(bar.build().role(Role::Row))
    }

    fn role(&self) -> CollectionRole {
        CollectionRole::Table
    }

    fn validate(&self) -> Result<(), CollectionError> {
        validate_table_structure(
            &self.columns,
            &self.header_styles,
            &self.row_styles,
            &self.cell_styles,
        )?;
        collect_keys(&self.items, self.key_fn.as_deref())?;
        check_row_height(self.estimated_row_height)?;
        Ok(())
    }
}
