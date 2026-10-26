//! Collection runtime: virtualized List/Table realization (runtime owner).
//!
//! Canonical home of the frozen builder/runtime seam from
//! `docs/COLLECTION_IMPLEMENTATION_CONTRACT.md`:
//!
//! ```rust,ignore
//! pub struct CollectionLayout { pub content_width: f32, pub column_widths: Vec<f32> }
//! pub enum CollectionRole { List, Table }
//! pub trait CollectionSource { ... }
//! pub struct CollectionDescriptor { ... }
//! ```
//!
//! `collection.rs` (collection-API worker) re-exports these exact items
//! (`pub use crate::collection_runtime::{...};`) so both owners share one
//! definition; the trait signatures below ARE the frozen seam. `Element`
//! and `Key` come from [`crate::element`]; `Style` from [`crate::style`].
//!
//! How it integrates (no new renderer primitive, no second graph):
//!
//! - [`crate::element::Element::collection`] lowers to ordinary retained
//!   nodes: a `Scroll` viewport (virtualized) or a `Box` column
//!   (non-virtualized), whose children the [`View`](crate::binding::View)
//!   keeps in sync with the visible window on every
//!   [`View::compute_layout`](crate::binding::View::compute_layout).
//! - Unmounted head/tail rows are represented by plain spacer `Box`es with
//!   explicit `Px` heights, so `Scroll` clamping, hit-testing, and Taffy
//!   layout all observe the true total content height with only the window
//!   mounted. No prebuild-all-then-hide, no fake flags.
//! - Measured committed row heights feed back into indexed prefix sums
//!   ([`crate::virtualization`]); layout settlement is bounded and errors
//!   surface through `View::compute_layout`.
//! - Keyed identity reuse, stable key + within-row reading anchors, focused /
//!   editing row pinning (mounted in isolation, never the whole gap), and
//!   per-row scope disposal all live here with the [`View`] driver.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::element::{lower_specs_in, Element, Key, LowerError, TypoChain};
use crate::style::{Axis, LayoutStyle, Length, Paint, Style};
use crate::tree::{BoxContent, NodeKind, NodeSpec};
use crate::virtualization::{window_for_viewport, PrefixSums, ReadingAnchor, RowRange};

/// Width plan for one layout pass. The source computes this from
/// `available_width` only (fit-to-content is explicit, never an implicit scan
/// of all virtual rows); each column's outer cell box enforces the shared
/// width. Header and body share one plan and one horizontal offset.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionLayout {
    /// Total content width in logical px.
    pub content_width: f32,
    /// Per-column widths in logical px (empty for lists).
    pub column_widths: Vec<f32>,
}

/// Whether a source describes a list or a table. Decides container/row/cell
/// semantics and whether a header row exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionRole {
    /// Single-column list.
    List,
    /// Multi-column table with a header.
    Table,
}

/// Validation/realization failures. Missing factories/keys, duplicate
/// keys/column ids, and invalid sizes/tracks fail at mount/layout through the
/// existing error seams, never silently omit rows or panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionError {
    /// A row factory is missing for the requested role/shape.
    MissingRowFactory,
    /// A key factory is missing (rows cannot be identified for reuse).
    MissingKeyFactory,
    /// Two rows map to the same key.
    DuplicateKey,
    /// Two table columns share one id.
    DuplicateColumnId,
    /// A width/track/size value is invalid (negative, NaN, ...).
    InvalidSize(String),
    /// A grid track is invalid outside table layout.
    InvalidTrack(String),
    /// Source-level validation message.
    Validation(String),
}

impl fmt::Display for CollectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectionError::MissingRowFactory => write!(f, "collection: missing row factory"),
            CollectionError::MissingKeyFactory => write!(f, "collection: missing key factory"),
            CollectionError::DuplicateKey => write!(f, "collection: duplicate row key"),
            CollectionError::DuplicateColumnId => write!(f, "collection: duplicate column id"),
            CollectionError::InvalidSize(s) => write!(f, "collection: invalid size: {s}"),
            CollectionError::InvalidTrack(s) => write!(f, "collection: invalid track: {s}"),
            CollectionError::Validation(s) => write!(f, "collection: invalid source: {s}"),
        }
    }
}

impl std::error::Error for CollectionError {}

/// Immutable snapshot data for one collection. Descriptors refresh through
/// the existing [`View`](crate::binding::View) reconciliation; the runtime
/// never mutates the source.
///
/// Implementors: `row()` must be cheap for mounted indices only — the runtime
/// calls it solely for the visible window (+ overscan + pinned rows), never
/// for all rows on scroll. `layout()` must not visit all cells; it may cache
/// a column plan by width.
pub trait CollectionSource {
    /// Row count in this snapshot.
    fn len(&self) -> usize;
    /// Whether this snapshot contains no rows.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Stable identity for row `index` (drives keyed reuse + anchors).
    fn key(&self, index: usize) -> Key;
    /// Width plan for `available_width`.
    fn layout(&self, available_width: f32) -> CollectionLayout;
    /// Row element for `index` under `layout` (key applied by the runtime —
    /// sources must NOT set it; the runtime overwrites with
    /// [`CollectionSource::key`] so reuse keys and anchor keys agree).
    fn row(&self, index: usize, layout: &CollectionLayout) -> Element;
    /// Header element under `layout` (tables; `None` for lists).
    fn header(&self, layout: &CollectionLayout) -> Option<Element>;
    /// List or table.
    fn role(&self) -> CollectionRole;
    /// Fail loudly on missing factories/keys, duplicate keys/column ids,
    /// invalid sizes/tracks. Called at mount and on descriptor refresh.
    fn validate(&self) -> Result<(), CollectionError>;
}

/// Mount descriptor: one immutable source snapshot plus presentation policy.
///
/// `collection.rs` builders produce these; `Element::collection` carries them
/// into the retained tree. Defaults: `estimated_row_height` 32 logical px,
/// `overscan` 4 rows per side, non-virtualized OFF unless `.virtualize()`.
pub struct CollectionDescriptor {
    /// Immutable snapshot source.
    pub source: Rc<dyn CollectionSource>,
    /// Container styles (normal [`Style`] merges, in order).
    pub styles: Vec<Style>,
    /// Virtualize (mount the scroll window only) vs mount every row.
    pub virtualized: bool,
    /// Per-row height guess until measured (logical px).
    pub estimated_row_height: f32,
    /// Mounted rows kept beyond the visible edges, per side.
    pub overscan: usize,
}

impl CollectionDescriptor {
    /// Build with explicit policy (builders fill this in). The estimated
    /// height is retained raw: non-finite/non-positive values fail explicit
    /// validation at mount
    /// ([`CollectionError::InvalidSize`]), never silently defaulted.
    pub fn new(
        source: Rc<dyn CollectionSource>,
        styles: Vec<Style>,
        virtualized: bool,
        estimated_row_height: f32,
        overscan: usize,
    ) -> Self {
        Self {
            source,
            styles,
            virtualized,
            estimated_row_height,
            overscan,
        }
    }
}

impl Clone for CollectionDescriptor {
    fn clone(&self) -> Self {
        Self {
            source: Rc::clone(&self.source),
            styles: self.styles.clone(),
            virtualized: self.virtualized,
            estimated_row_height: self.estimated_row_height,
            overscan: self.overscan,
        }
    }
}

impl fmt::Debug for CollectionDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CollectionDescriptor")
            .field("rows", &self.source.len())
            .field("styles", &self.styles.len())
            .field("virtualized", &self.virtualized)
            .field("estimated_row_height", &self.estimated_row_height)
            .field("overscan", &self.overscan)
            .finish()
    }
}

/// Validate a source snapshot: run its own checks, verify the estimated
/// height is a finite positive size, then verify key uniqueness with one
/// full `O(n)` key pass (no row factories invoked). Structural refreshes pay
/// linear validation once; steady-state scrolling stays `O(log n +
/// mounted)`. A duplicate anywhere is a hard error, never a silent merge.
pub fn validate_snapshot(source: &dyn CollectionSource) -> Result<(), CollectionError> {
    source.validate()?;
    let len = source.len();
    if len == 0 {
        return Ok(());
    }
    // Hash every key exactly once; a repeat is a hard error, not a silent
    // merge. Full pass over all rows — no truncation: deep duplicate keys
    // must fail just as loudly as shallow ones.
    let mut seen = std::collections::HashSet::with_capacity(len);
    for i in 0..len {
        let k = crate::tree::hash_key(source.key(i));
        if !seen.insert(k) {
            return Err(CollectionError::DuplicateKey);
        }
    }
    Ok(())
}

/// Validate the descriptor policy itself (estimated height must be a finite
/// positive size so prefix sums stay exact). Sources validate their own
/// factories/keys/tracks through [`CollectionSource::validate`].
pub fn validate_descriptor(desc: &CollectionDescriptor) -> Result<(), CollectionError> {
    if !(desc.estimated_row_height.is_finite() && desc.estimated_row_height > 0.0) {
        return Err(CollectionError::InvalidSize(format!(
            "estimated_row_height must be finite and positive, got {}",
            desc.estimated_row_height
        )));
    }
    Ok(())
}

/// Resolve the collection container's `(LayoutStyle, Paint)` from the
/// descriptor's styles in order (normal [`Style`] merges; later wins per
/// field). The container stacks vertically by default (authors may override
/// the axis through styles); horizontal table scrolling rides on the shared
/// scroll offset, not on container restyling.
#[must_use]
pub fn container_style(desc: &CollectionDescriptor) -> (LayoutStyle, Paint) {
    let mut layout = LayoutStyle {
        axis: Axis::Column,
        ..Default::default()
    };
    let mut paint = Paint::default();
    for s in &desc.styles {
        s.apply_to(&mut layout, &mut paint);
    }
    // The same viewport defaults apply on mount AND refresh. Otherwise a
    // refreshed Auto root expands to its spacer total and defeats clipping.
    if desc.virtualized {
        if layout.height == Length::Auto {
            layout.height = Length::Pct(100.0);
        }
        if layout.width == Length::Auto {
            layout.width = Length::Pct(100.0);
        }
    }
    (layout, paint)
}

/// Authored inter-row gap for the body stack, resolved from the descriptor
/// styles in order (explicit `row_gap` wins, else `gap`). The driver folds
/// this gap into every row stride and forces the internal body stack's Taffy
/// gap to zero, so spacer math is exact: spacers are ordinary siblings and
/// would otherwise inherit a phantom extra gap and drift deep in the list.
/// Visually identical to a full layout with the authored gap — verified by
/// the gap+margin deep-scroll tests.
#[must_use]
pub fn body_gap(desc: &CollectionDescriptor) -> f32 {
    let (layout, _) = container_style(desc);
    let g = layout.row_gap.unwrap_or(layout.gap);
    if g.is_finite() && g >= 0.0 {
        g
    } else {
        0.0
    }
}
/// Shell shape for a descriptor (agreed contract with the View driver owner,
/// via parent): tells the driver what to BUILD into the empty lowered shell.
/// Lowering never runs row/header factories and never emits row children —
/// the driver owns ALL realization for both virtualized and non-virtualized
/// modes, mounting rows and binding exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellShape {
    /// List: the shell container itself is the body. Virtualized → `Scroll`
    /// viewport; non-virtualized → `Box` column. The driver syncs row/spacer
    /// children directly into the container (all rows when non-virtualized).
    ListBody,
    /// Table (BOTH modes): the shell is an empty `Box` column root. The
    /// driver creates exactly two children in order — `[header, body]` —
    /// sharing the one width plan, with the header outside the vertical
    /// body viewport (clipping guaranteed) mirroring the body's horizontal
    /// scroll offset. Bodies are `Scroll` viewports in both modes; only
    /// row realization is virtualized. Normal tables mount every row.
    /// Table roots are never `Scroll` (the header would clip wrong).
    TableSplit,
}

/// Shell shape for a descriptor: every table splits (both modes), lists are
/// a single body container.
#[must_use]
pub fn shell_shape(desc: &CollectionDescriptor) -> ShellShape {
    if desc.source.role() == CollectionRole::Table {
        ShellShape::TableSplit
    } else {
        ShellShape::ListBody
    }
}

/// First-paint available width guess for the column plan, used only until
/// the first committed viewport arrives. The driver recomputes the plan from
/// the real viewport width on every `compute_layout` and rebuilds rows when
/// it changes — this constant never governs steady state.
pub const INITIAL_AVAILABLE_WIDTH: f32 = 800.0;

/// Lower one source row to specs with the runtime-owned identity key forced
/// on top (sources must not set keys; reuse keys and anchor keys must
/// agree). DRIVER-ONLY helper: the driver calls this once per mounted row
/// when it realizes and binds. Lowering itself never calls it (shell-only;
/// factories run exactly once, inside the driver).
pub fn lower_row_specs(
    source: &dyn CollectionSource,
    index: usize,
    plan: &CollectionLayout,
    inherited: &TypoChain,
) -> Result<Vec<NodeSpec>, LowerError> {
    let row = source.row(index, plan).with_key(source.key(index));
    lower_specs_in(&row, inherited)
}

/// Lower the table header (if any) to specs. DRIVER-ONLY helper, like
/// [`lower_row_specs`]: lowering never calls factories. Header specs stay
/// unkeyed: they are positional children ahead of the keyed rows, so refresh
/// reuses them positionally without ever colliding with row keys.
pub fn lower_header_specs(
    source: &dyn CollectionSource,
    plan: &CollectionLayout,
    inherited: &TypoChain,
) -> Result<Vec<NodeSpec>, LowerError> {
    match source.header(plan) {
        Some(h) => lower_specs_in(&h, inherited),
        None => Ok(Vec::new()),
    }
}

/// Plain spacer box of exact `height`: represents unmounted head/tail/gap
/// rows so `Scroll` clamping, hit-testing, and Taffy all observe the true
/// total content height with only the window mounted. No state, no
/// bindings — pure geometry.
pub fn spacer_spec(height: f32) -> NodeSpec {
    let style = LayoutStyle {
        height: Length::Px(height.max(0.0)),
        flex_shrink: 0.0,
        flex_grow: 0.0,
        ..Default::default()
    };
    NodeSpec::new(NodeKind::Box(BoxContent::default()), style)
}

/// Per-mount virtualization state for one collection container.
///
/// Owns the indexed prefix sums (Fenwick-backed, `O(log n)` updates/lookups),
/// the viewport + scroll inputs, the reading anchor, and the focused/editing
/// pin. The [`View`](crate::binding::View) driver feeds it committed rects
/// and scroll offsets; it answers which indices to mount.
pub struct CollectionState {
    desc: CollectionDescriptor,
    /// Authoritative per-row content heights (estimates until measured).
    heights: Vec<f32>,
    /// Per-row vertical margins `(top, bottom)` from committed node styles
    /// (zero until measured). Folded into strides so margins never drift.
    margins: Vec<(f32, f32)>,
    /// Authored inter-row gap folded into every stride (see [`body_gap`]).
    gap: f32,
    /// Indexed prefix sums over row *strides*
    /// (`height + margin_top + margin_bottom + gap`).
    sums: PrefixSums,
    /// Last committed viewport width (drives the column plan).
    viewport_w: f32,
    /// Last committed viewport height (drives the vertical window).
    viewport_h: f32,
    /// Current scroll offsets (body; header mirrors x).
    scroll_x: f32,
    /// Current scroll offsets (body vertical).
    scroll_y: f32,
    /// Row the viewport is anchored to: (key hash, offset within row).
    anchor: Option<(u64, f32)>,
    /// Pinned row key hash (focused/editing row stays mounted in isolation).
    pinned: Option<u64>,
    /// Key hash → current index, rebuilt on descriptor refresh.
    index_by_key: HashMap<u64, usize>,
    /// Currently mounted window (last realized).
    mounted: RowRange,
    /// Committed column plan for the current `viewport_w`.
    plan: CollectionLayout,
    /// Rows whose measured heights changed since the last settle pass.
    dirty_heights: bool,
}

impl CollectionState {
    /// Create state for a freshly mounted descriptor (validates eagerly).
    pub fn new(desc: CollectionDescriptor) -> Result<Self, CollectionError> {
        validate_descriptor(&desc)?;
        validate_snapshot(desc.source.as_ref())?;
        let len = desc.source.len();
        let est = desc.estimated_row_height;
        let gap = body_gap(&desc);
        let heights = vec![est; len];
        let margins = vec![(0.0f32, 0.0f32); len];
        let sums = Self::build_sums(&heights, &margins, gap);
        let mut st = Self {
            plan: desc.source.layout(INITIAL_AVAILABLE_WIDTH),
            desc,
            heights,
            margins,
            gap,
            sums,
            viewport_w: 0.0,
            viewport_h: 0.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            anchor: None,
            pinned: None,
            index_by_key: HashMap::new(),
            mounted: RowRange::empty(0),
            dirty_heights: false,
        };
        st.rebuild_index();
        Ok(st)
    }

    /// Stride of one row: content height plus vertical margins plus the
    /// authored inter-row gap. Strides are what Taffy stacks, so prefix sums
    /// over strides match committed geometry exactly — including gap and
    /// margins — with no phantom drift deep in the list.
    fn stride_of(heights: &[f32], margins: &[(f32, f32)], gap: f32, i: usize) -> f32 {
        heights[i] + margins[i].0 + margins[i].1 + gap
    }

    fn build_sums(heights: &[f32], margins: &[(f32, f32)], gap: f32) -> PrefixSums {
        let strides: Vec<f32> = (0..heights.len())
            .map(|i| Self::stride_of(heights, margins, gap, i))
            .collect();
        PrefixSums::build(&strides)
    }

    /// Swap in a refreshed descriptor snapshot, preserving heights AND
    /// margins by key, the reading anchor, and the pin. Unknown rows fall
    /// back to the new estimate with zero margins. Full `O(n)` key passes —
    /// structural refreshes pay linear work once; steady-state scrolling
    /// stays `O(log n + mounted)`.
    pub fn refresh(&mut self, desc: CollectionDescriptor) -> Result<(), CollectionError> {
        validate_descriptor(&desc)?;
        validate_snapshot(desc.source.as_ref())?;
        let new_len = desc.source.len();
        // Capture the anchor against the OLD sums before swapping.
        let anchor_key = self.anchor_key_at_scroll();
        // Preserve every measured height AND margin by key across the
        // refresh — no truncation: deep rows keep measurements and anchors.
        let mut old_by_key: HashMap<u64, (f32, (f32, f32))> =
            HashMap::with_capacity(self.heights.len());
        for (i, h) in self.heights.iter().enumerate() {
            if let Some(k) = self.key_at(i) {
                old_by_key.insert(k, (*h, self.margins[i]));
            }
        }
        let est = desc.estimated_row_height;
        let gap = body_gap(&desc);
        let mut heights = Vec::with_capacity(new_len);
        let mut margins = Vec::with_capacity(new_len);
        for i in 0..new_len {
            let k = crate::tree::hash_key(desc.source.key(i));
            let (h, m) = old_by_key.get(&k).copied().unwrap_or((est, (0.0, 0.0)));
            heights.push(h);
            margins.push(m);
        }
        self.gap = gap;
        self.sums = Self::build_sums(&heights, &margins, gap);
        self.heights = heights;
        self.margins = margins;
        self.desc = desc;
        self.rebuild_index();
        // Re-resolve the anchor: same key + within-row offset, new index.
        if let Some((k, off)) = anchor_key {
            self.anchor = Some((k, off));
            if let Some(&idx) = self.index_by_key.get(&k) {
                let a = ReadingAnchor {
                    key: k,
                    offset_in_row: off,
                };
                self.scroll_y = a.restore_scroll(&self.sums, idx, self.viewport_h);
            }
        }
        // A reordered/rekeyed snapshot invalidates the old pin when the key
        // is gone; otherwise the pin follows the key, not the position.
        if let Some(p) = self.pinned {
            if !self.index_by_key.contains_key(&p) {
                self.pinned = None;
            }
        }
        self.dirty_heights = true;
        Ok(())
    }

    fn key_at(&self, index: usize) -> Option<u64> {
        if index < self.desc.source.len() {
            Some(crate::tree::hash_key(self.desc.source.key(index)))
        } else {
            None
        }
    }

    fn rebuild_index(&mut self) {
        self.index_by_key.clear();
        let len = self.desc.source.len();
        // Full O(n) index on every structural refresh: steady-state scrolls
        // are pure O(log n) lookups afterwards. No truncation.
        self.index_by_key.reserve(len);
        for i in 0..len {
            self.index_by_key
                .insert(crate::tree::hash_key(self.desc.source.key(i)), i);
        }
    }

    /// Key hash → current index (`O(1)`; the index covers every row — no
    /// per-scroll scans of any kind).
    pub fn index_of(&self, key: u64) -> Option<usize> {
        self.index_by_key.get(&key).copied()
    }

    /// Descriptor borrow (row factories, role, policy).
    pub fn descriptor(&self) -> &CollectionDescriptor {
        &self.desc
    }

    /// Current column plan.
    pub fn plan(&self) -> &CollectionLayout {
        &self.plan
    }

    /// Recompute the column plan when the viewport width changed. Returns
    /// true when the plan is new (rows must rebuild, heights re-measured).
    pub fn set_viewport(&mut self, w: f32, h: f32) -> bool {
        let width_changed = (w - self.viewport_w).abs() > f32::EPSILON;
        self.viewport_w = w;
        self.viewport_h = h;
        if width_changed && w > 0.0 {
            self.plan = self.desc.source.layout(w);
            self.dirty_heights = true;
            true
        } else {
            false
        }
    }

    /// Feed the committed body scroll offsets (the single source of truth is
    /// the retained `Scroll` node's offset; this just caches it).
    /// Non-finite inputs are ignored (window math sanitizes anyway, but the
    /// anchor accounting must never latch onto NaN).
    pub fn set_scroll(&mut self, x: f32, y: f32) {
        if x.is_finite() {
            self.scroll_x = x;
        }
        if y.is_finite() {
            self.scroll_y = y;
        }
    }

    /// Current body scroll.
    pub fn scroll(&self) -> (f32, f32) {
        (self.scroll_x, self.scroll_y)
    }

    /// Pin a row key (focused/editing row). `None` clears the pin.
    pub fn set_pinned_key(&mut self, key: Option<u64>) {
        self.pinned = key;
    }

    /// Record the committed geometry of a mounted row: content height plus
    /// vertical margins from the committed node style. Returns true when the
    /// stride changed (marks the sums dirty for one bounded re-settle).
    ///
    /// Exact-equality comparison on the stride: committed rects are
    /// deterministic, so a settled row stops immediately with no oscillation,
    /// while even fractional rows are tracked exactly (small deltas
    /// accumulate across rows; a coarse threshold would drift the total).
    /// Zero-height (collapsed) rows are legitimate; non-finite or negative
    /// heights are rejected and keep the old value.
    ///
    /// Negative margins are accepted ONLY when the resulting stride stays
    /// finite and non-negative: prefix sums require monotonic non-decreasing
    /// partial sums, and a negative stride would break index lookups. A
    /// measurement that would drive the stride negative (or non-finite) is
    /// rejected and keeps the old geometry.
    ///
    /// Anchor preservation: when a row entirely above the viewport top
    /// changes stride, the scroll offset moves by the same delta so the
    /// content under the viewport stays put (same guarantee as
    /// [`CollectionState::refresh`], applied continuously during settle —
    /// covers measured-height changes above the anchor, not just refreshes).
    pub fn note_measured(&mut self, index: usize, h: f32, mt: f32, mb: f32) -> bool {
        if index >= self.heights.len() || !h.is_finite() || h < 0.0 {
            return false;
        }
        let mt = if mt.is_finite() { mt } else { 0.0 };
        let mb = if mb.is_finite() { mb } else { 0.0 };
        let new_stride = h + mt + mb + self.gap;
        if !new_stride.is_finite() || new_stride < 0.0 {
            // Would break Fenwick monotonicity: keep the old geometry.
            return false;
        }
        let old_stride =
            self.heights[index] + self.margins[index].0 + self.margins[index].1 + self.gap;
        if old_stride == new_stride {
            return false;
        }
        let delta = new_stride - old_stride;
        self.heights[index] = h;
        self.margins[index] = (mt, mb);
        self.sums.add(index, delta as f64);
        let row_top = self.sums.prefix(index) as f32;
        if row_top + old_stride <= self.scroll_y {
            // Entirely above the viewport top: shift the scroll with it.
            self.scroll_y = (self.scroll_y + delta).max(0.0);
        }
        self.dirty_heights = true;
        true
    }

    /// Whether any measured height is still unsettled.
    pub fn heights_dirty(&self) -> bool {
        self.dirty_heights
    }

    /// Mark the settle pass complete.
    pub fn mark_settled(&mut self) {
        self.dirty_heights = false;
    }

    /// Total content stride from indexed sums (includes trailing gap per
    /// row; the laid-out extent is `total - gap` for non-empty lists).
    pub fn total_height(&self) -> f32 {
        self.sums.total() as f32
    }

    /// Stride offset of row `index` (its laid-out top minus its own top
    /// margin — Taffy adds the margin from the row's own style).
    pub fn offset_of(&self, index: usize) -> f32 {
        self.sums.prefix(index.min(self.heights.len())) as f32
    }

    /// Authored inter-row gap folded into every stride.
    pub fn gap(&self) -> f32 {
        self.gap
    }

    /// Exact laid-out height of the span `[start, end)`: summed strides
    /// minus one inter-row gap (spans never carry a trailing gap). Empty
    /// spans are zero. This is the spacer height that reproduces full-layout
    /// geometry exactly — spacers are ordinary siblings with zero margins,
    /// so Taffy adds exactly one gap on each side, matching the folded
    /// accounting.
    pub fn span_height(&self, start: usize, end: usize) -> f32 {
        let (s, e) = (start.min(end), end.max(start));
        let len = self.heights.len();
        if s >= e || s >= len {
            return 0.0;
        }
        let e = e.min(len);
        let sum = self.sums.prefix(e) - self.sums.prefix(s);
        (sum as f32 - self.gap).max(0.0)
    }

    /// Re-resolve the authored gap from fresh descriptor styles (style-only
    /// refresh): rebuilds stride sums in one `O(n)` pass — same order as a
    /// structural refresh, never per scroll.
    pub fn restyle_gap(&mut self) {
        let gap = body_gap(&self.desc);
        if gap != self.gap {
            self.gap = gap;
            self.sums = Self::build_sums(&self.heights, &self.margins, gap);
            self.dirty_heights = true;
        }
    }

    /// Last realized window.
    pub fn mounted(&self) -> RowRange {
        self.mounted
    }

    /// Compute the window to mount: visible rows + bounded overscan, plus the
    /// pinned row in isolation when it falls outside. Non-virtualized mode
    /// mounts every row through the same factories. Zero viewport mounts a
    /// minimal anchor window, never all rows.
    pub fn desired_window(&self) -> RowRange {
        let len = self.desc.source.len();
        if len == 0 {
            return RowRange::empty(0);
        }
        if !self.desc.virtualized {
            return RowRange { start: 0, end: len };
        }
        window_for_viewport(
            &self.sums,
            len,
            self.scroll_y,
            self.viewport_h,
            self.desc.overscan,
        )
    }

    /// Sorted mounted index runs: the desired window plus the pinned row as
    /// an isolated one-row run when outside the window. Gaps between runs
    /// become spacer boxes — the pinned row never drags the whole gap into
    /// the tree.
    pub fn mounted_runs(&self, window: RowRange) -> Vec<RowRange> {
        let mut runs = vec![window];
        if let Some(p) = self.pinned {
            if let Some(idx) = self.index_of(p) {
                if !window.contains(idx) {
                    runs.push(RowRange {
                        start: idx,
                        end: idx + 1,
                    });
                }
            }
        }
        runs.sort_by_key(|r| r.start);
        runs
    }

    /// Realize `window` as the mounted set (records it for `mounted()`).
    pub fn set_mounted(&mut self, window: RowRange) {
        self.mounted = window;
    }

    /// Key hash of the row under the current scroll top (anchor capture).
    fn anchor_key_at_scroll(&self) -> Option<(u64, f32)> {
        let len = self.desc.source.len();
        if len == 0 {
            return None;
        }
        let idx = self.sums.index_at(self.scroll_y as f64).min(len - 1);
        let top = self.sums.prefix(idx) as f32;
        Some((
            crate::tree::hash_key(self.desc.source.key(idx)),
            (self.scroll_y - top).max(0.0),
        ))
    }

    /// Capture the current reading anchor (called before a refresh that may
    /// move rows, e.g. prepend/reorder).
    pub fn capture_anchor(&mut self) {
        self.anchor = self.anchor_key_at_scroll();
    }

    /// Current reading anchor: (row key hash, offset within the row from
    /// its top edge). The driver restores it after structural changes via
    /// [`ReadingAnchor::restore_scroll`]; [`CollectionState::refresh`] and
    /// [`CollectionState::note_measured`] maintain it automatically.
    pub fn anchor(&self) -> Option<(u64, f32)> {
        self.anchor
    }

    /// Style-only refresh: swap the descriptor's style list (same source
    /// snapshot, e.g. theme change) and re-resolve the authored gap. One
    /// `O(n)` stride rebuild, never per scroll. The column plan, window,
    /// scroll, anchor, pin, and measured heights/margins are preserved.
    /// For a new source snapshot use [`CollectionState::refresh`].
    pub fn update_styles(&mut self, styles: Vec<Style>) {
        self.desc.styles = styles;
        self.restyle_gap();
    }

    /// Pinned row key hash, if any (focused/editing row).
    pub fn pinned_key(&self) -> Option<u64> {
        self.pinned
    }

    /// Number of live rows in this snapshot.
    pub fn len(&self) -> usize {
        self.desc.source.len()
    }

    /// Whether the snapshot is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::ui;

    struct FakeList {
        n: usize,
    }

    impl CollectionSource for FakeList {
        fn len(&self) -> usize {
            self.n
        }
        fn key(&self, index: usize) -> Key {
            Key::Usize(index)
        }
        fn layout(&self, available_width: f32) -> CollectionLayout {
            CollectionLayout {
                content_width: available_width,
                column_widths: Vec::new(),
            }
        }
        fn row(&self, index: usize, _layout: &CollectionLayout) -> Element {
            ui::Text::builder().arg(format!("row-{index}")).build()
        }
        fn header(&self, _layout: &CollectionLayout) -> Option<Element> {
            None
        }
        fn role(&self) -> CollectionRole {
            CollectionRole::List
        }
        fn validate(&self) -> Result<(), CollectionError> {
            Ok(())
        }
    }

    fn desc(n: usize, virtualized: bool, est: f32, overscan: usize) -> CollectionDescriptor {
        CollectionDescriptor::new(
            std::rc::Rc::new(FakeList { n }),
            Vec::new(),
            virtualized,
            est,
            overscan,
        )
    }

    fn gapped(n: usize) -> CollectionDescriptor {
        CollectionDescriptor::new(
            std::rc::Rc::new(FakeList { n }),
            vec![Style::new().column().gap(4.0)],
            true,
            32.0,
            4,
        )
    }

    #[test]
    fn strides_fold_gap_and_margins_exactly() {
        let mut st = CollectionState::new(gapped(100)).unwrap();
        // Estimate stride: 32 + 0 + 0 + 4.
        assert_eq!(st.gap(), 4.0);
        assert!((st.offset_of(10) - 10.0 * 36.0).abs() < 1e-3);
        // Span drops the one trailing gap.
        assert!((st.span_height(0, 10) - (10.0 * 36.0 - 4.0)).abs() < 1e-3);
        assert_eq!(st.span_height(5, 5), 0.0);
        assert_eq!(st.span_height(0, 0), 0.0);
        // Measured margins fold in exactly.
        assert!(st.note_measured(0, 32.0, 2.0, 3.0));
        assert!((st.offset_of(1) - (32.0 + 2.0 + 3.0 + 4.0)).abs() < 1e-3);
        // Settled rows report no change.
        assert!(!st.note_measured(0, 32.0, 2.0, 3.0));
    }

    #[test]
    fn zero_fractional_and_negative_margin_strides() {
        let mut st = CollectionState::new(desc(10, true, 32.0, 1)).unwrap();
        // Collapsed rows are legitimate.
        assert!(st.note_measured(3, 0.0, 0.0, 0.0));
        // Fractional strides track exactly.
        assert!(st.note_measured(4, 32.25, 0.0, 0.0));
        assert!(!st.note_measured(4, 32.25, 0.0, 0.0));
        // Negative margins hold while the stride stays >= 0.
        assert!(st.note_measured(5, 32.0, -4.0, 0.0));
        // A stride driven negative is rejected; old geometry stays.
        let before = st.offset_of(6);
        assert!(!st.note_measured(5, 1.0, -4.0, 0.0));
        assert_eq!(st.offset_of(6), before);
        // Non-finite heights rejected.
        assert!(!st.note_measured(6, f32::NAN, 0.0, 0.0));
        assert!(!st.note_measured(6, f32::INFINITY, 0.0, 0.0));
        assert!(!st.note_measured(6, -1.0, 0.0, 0.0));
        assert!(!st.note_measured(999, 10.0, 0.0, 0.0));
    }

    #[test]
    fn height_change_above_anchor_shifts_scroll() {
        let mut st = CollectionState::new(gapped(50)).unwrap();
        st.set_viewport(800.0, 600.0);
        // Scroll deep: row tops are at multiples of 36.
        st.set_scroll(0.0, 360.0);
        // Row 0 (stride 36) is entirely above: growing it shifts scroll 1:1.
        assert!(st.note_measured(0, 40.0, 0.0, 0.0));
        assert!((st.scroll().1 - 368.0).abs() < 1e-3);
        // The anchor row under the viewport keeps its offset within the row:
        // prefix math stays consistent (no drift between sums and scroll).
        let idx = st.sums.index_at(st.scroll_y as f64);
        let top = st.sums.prefix(idx) as f32;
        assert!(st.scroll_y >= top);
    }

    #[test]
    fn refresh_preserves_measured_heights_by_key() {
        let mut st = CollectionState::new(gapped(20)).unwrap();
        assert!(st.note_measured(7, 50.0, 1.0, 1.0));
        let stride_before = st.offset_of(8) - st.offset_of(7);
        st.refresh(gapped(20)).unwrap();
        let stride_after = st.offset_of(8) - st.offset_of(7);
        assert!(
            (stride_before - stride_after).abs() < 1e-6,
            "{stride_before} vs {stride_after}"
        );
        assert!(st.heights_dirty());
    }

    #[test]
    fn invalid_estimate_and_duplicate_keys_fail() {
        assert!(matches!(
            CollectionState::new(desc(10, true, f32::NAN, 4)),
            Err(CollectionError::InvalidSize(_))
        ));
        assert!(matches!(
            CollectionState::new(desc(10, true, 0.0, 4)),
            Err(CollectionError::InvalidSize(_))
        ));
        struct Dup;
        impl CollectionSource for Dup {
            fn len(&self) -> usize {
                4
            }
            fn key(&self, _: usize) -> Key {
                Key::Usize(1)
            }
            fn layout(&self, w: f32) -> CollectionLayout {
                CollectionLayout {
                    content_width: w,
                    column_widths: Vec::new(),
                }
            }
            fn row(&self, i: usize, _: &CollectionLayout) -> Element {
                ui::Text::builder().arg(format!("{i}")).build()
            }
            fn header(&self, _: &CollectionLayout) -> Option<Element> {
                None
            }
            fn role(&self) -> CollectionRole {
                CollectionRole::List
            }
            fn validate(&self) -> Result<(), CollectionError> {
                Ok(())
            }
        }
        assert!(matches!(
            CollectionState::new(CollectionDescriptor::new(
                std::rc::Rc::new(Dup),
                Vec::new(),
                true,
                32.0,
                4
            )),
            Err(CollectionError::DuplicateKey)
        ));
    }

    #[test]
    fn shell_shapes_split_all_tables() {
        struct T;
        impl CollectionSource for T {
            fn len(&self) -> usize {
                5
            }
            fn key(&self, i: usize) -> Key {
                Key::Usize(i)
            }
            fn layout(&self, w: f32) -> CollectionLayout {
                CollectionLayout {
                    content_width: w,
                    column_widths: vec![w],
                }
            }
            fn row(&self, i: usize, _: &CollectionLayout) -> Element {
                ui::Text::builder().arg(format!("{i}")).build()
            }
            fn header(&self, _: &CollectionLayout) -> Option<Element> {
                Some(ui::Text::builder().arg("h").build())
            }
            fn role(&self) -> CollectionRole {
                CollectionRole::Table
            }
            fn validate(&self) -> Result<(), CollectionError> {
                Ok(())
            }
        }
        for virtualized in [true, false] {
            let d =
                CollectionDescriptor::new(std::rc::Rc::new(T), Vec::new(), virtualized, 32.0, 4);
            assert_eq!(shell_shape(&d), ShellShape::TableSplit);
        }
        assert_eq!(shell_shape(&desc(5, true, 32.0, 4)), ShellShape::ListBody);
        assert_eq!(shell_shape(&desc(5, false, 32.0, 4)), ShellShape::ListBody);
    }
}
