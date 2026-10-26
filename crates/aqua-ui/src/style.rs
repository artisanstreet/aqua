//! Typed styles. Application code uses these structs; [`to_taffy`] maps the
//! layout subset to `taffy::Style` at the layout boundary. No CSS strings,
//! no stringly-typed properties. Paint properties travel to scene emission.

use aqua_core::{CornerRadii, Insets, Oklch};
use taffy::prelude::TaffyAuto as _;
use taffy::style::{
    AlignContent, AlignItems, AlignSelf, Dimension, Display, FlexDirection, FlexWrap,
    JustifyContent, LengthPercentage, LengthPercentageAuto, MaxTrackSizingFunction,
    MinTrackSizingFunction, Position,
};

/// One-dimensional length: fixed logical px, percent of the parent axis, or
/// content-driven (`Auto`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    /// Fixed logical pixels.
    Px(f32),
    /// Percent (0..100+) of the parent axis.
    Pct(f32),
    /// Content-sized.
    Auto,
}

impl Length {
    fn to_lp(self) -> LengthPercentage {
        match self {
            Length::Px(v) => LengthPercentage::length(v),
            Length::Pct(p) => LengthPercentage::percent(p / 100.0),
            Length::Auto => LengthPercentage::length(0.0),
        }
    }

    fn to_dimension(self) -> Dimension {
        match self {
            Length::Px(v) => Dimension::length(v),
            Length::Pct(p) => Dimension::percent(p / 100.0),
            Length::Auto => Dimension::AUTO,
        }
    }

    fn to_lpa(self) -> LengthPercentageAuto {
        match self {
            Length::Px(v) => LengthPercentageAuto::length(v),
            Length::Pct(p) => LengthPercentageAuto::percent(p / 100.0),
            Length::Auto => LengthPercentageAuto::AUTO,
        }
    }
}

/// Main-axis direction for flex containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Axis {
    /// Horizontal row (default).
    #[default]
    Row,
    /// Vertical column.
    Column,
}

/// Layout properties for every node. Flex rows/columns, grid, sizing
/// constraints, spacing, alignment, and explicit positioning are one model;
/// rows/columns/grids are configurations, not separate element families.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutStyle {
    /// Width (default content-sized).
    pub width: Length,
    /// Height (default content-sized).
    pub height: Length,
    /// Minimum width.
    pub min_width: Length,
    /// Minimum height.
    pub min_height: Length,
    /// Maximum width (`Auto` = unbounded).
    pub max_width: Length,
    /// Maximum height (`Auto` = unbounded).
    pub max_height: Length,
    /// Flex grow factor.
    pub flex_grow: f32,
    /// Flex shrink factor.
    pub flex_shrink: f32,
    /// Main axis.
    pub axis: Axis,
    /// Whether children wrap.
    pub wrap: bool,
    /// Cross-axis alignment of children.
    pub align: AlignItems,
    /// Self alignment override (`None` = auto, inherits the container).
    pub align_self: Option<AlignSelf>,
    /// Main-axis distribution.
    pub justify: JustifyContent,
    /// Gap between children (both axes unless overridden).
    pub gap: f32,
    /// Column gap override (`None` = `gap`).
    pub column_gap: Option<f32>,
    /// Row gap override (`None` = `gap`).
    pub row_gap: Option<f32>,
    /// Inner spacing.
    pub padding: Insets,
    /// Outer spacing.
    pub margin: Insets,
    /// Grid columns (`None` = flex layout). Entries are track sizes.
    pub grid_columns: Option<Vec<Length>>,
    /// Grid rows (`None` = flex layout / auto).
    pub grid_rows: Option<Vec<Length>>,
    /// Absolute positioning (removed from flow, offset from padding box).
    pub absolute: Option<Absolute>,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        Self {
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            min_height: Length::Auto,
            max_width: Length::Auto,
            max_height: Length::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            axis: Axis::Row,
            wrap: false,
            align: AlignItems::STRETCH,
            align_self: None,
            justify: JustifyContent::START,
            gap: 0.0,
            column_gap: None,
            row_gap: None,
            padding: Insets::default(),
            margin: Insets::default(),
            grid_columns: None,
            grid_rows: None,
            absolute: None,
        }
    }
}

/// Absolute offset from the containing block's padding box.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Absolute {
    /// Left offset (`None` = unset).
    pub left: Option<f32>,
    /// Top offset.
    pub top: Option<f32>,
    /// Right offset.
    pub right: Option<f32>,
    /// Bottom offset.
    pub bottom: Option<f32>,
}

/// Convert the typed style to `taffy::Style`. Grid tracks become single
/// (non-repeated) track sizing functions; flex is the default display.
#[must_use]
pub fn to_taffy(s: &LayoutStyle) -> taffy::Style {
    use taffy::style::{GridTemplateComponent, TrackSizingFunction};
    let track = |t: Length| {
        let (min, max) = match t {
            Length::Px(v) => (
                MinTrackSizingFunction::length(v),
                MaxTrackSizingFunction::length(v),
            ),
            Length::Pct(p) => (
                MinTrackSizingFunction::percent(p / 100.0),
                MaxTrackSizingFunction::percent(p / 100.0),
            ),
            Length::Auto => (MinTrackSizingFunction::AUTO, MaxTrackSizingFunction::AUTO),
        };
        GridTemplateComponent::Single(TrackSizingFunction { min, max })
    };
    let rect = |i: Insets| taffy::geometry::Rect {
        left: LengthPercentage::length(i.left),
        right: LengthPercentage::length(i.right),
        top: LengthPercentage::length(i.top),
        bottom: LengthPercentage::length(i.bottom),
    };
    let display = if s.grid_columns.is_some() || s.grid_rows.is_some() {
        taffy::style::Display::Grid
    } else {
        Display::Flex
    };
    taffy::Style {
        display,
        size: taffy::geometry::Size {
            width: s.width.to_dimension(),
            height: s.height.to_dimension(),
        },
        min_size: taffy::geometry::Size {
            width: s.min_width.to_lpa(),
            height: s.min_height.to_lpa(),
        },
        max_size: taffy::geometry::Size {
            width: s.max_width.to_lpa(),
            height: s.max_height.to_lpa(),
        },
        flex_grow: s.flex_grow,
        flex_shrink: s.flex_shrink,
        flex_direction: match s.axis {
            Axis::Row => FlexDirection::Row,
            Axis::Column => FlexDirection::Column,
        },
        flex_wrap: if s.wrap {
            FlexWrap::Wrap
        } else {
            FlexWrap::NoWrap
        },
        align_items: s.align,
        align_self: s.align_self,
        align_content: AlignContent::START,
        justify_content: s.justify,
        gap: taffy::geometry::Size {
            width: Length::Px(s.column_gap.unwrap_or(s.gap)).to_lp(),
            height: Length::Px(s.row_gap.unwrap_or(s.gap)).to_lp(),
        },
        padding: rect(s.padding),
        margin: taffy::geometry::Rect {
            left: LengthPercentageAuto::length(s.margin.left),
            right: LengthPercentageAuto::length(s.margin.right),
            top: LengthPercentageAuto::length(s.margin.top),
            bottom: LengthPercentageAuto::length(s.margin.bottom),
        },
        grid_template_columns: s
            .grid_columns
            .as_ref()
            .map(|tracks| tracks.iter().map(|t| track(*t)).collect())
            .unwrap_or_default(),
        grid_template_rows: s
            .grid_rows
            .as_ref()
            .map(|tracks| tracks.iter().map(|t| track(*t)).collect())
            .unwrap_or_default(),
        position: if s.absolute.is_some() {
            Position::Absolute
        } else {
            Position::Relative
        },
        inset: taffy::geometry::Rect {
            left: s
                .absolute
                .and_then(|a| a.left)
                .map_or(LengthPercentageAuto::AUTO, LengthPercentageAuto::length),
            right: s
                .absolute
                .and_then(|a| a.right)
                .map_or(LengthPercentageAuto::AUTO, LengthPercentageAuto::length),
            top: s
                .absolute
                .and_then(|a| a.top)
                .map_or(LengthPercentageAuto::AUTO, LengthPercentageAuto::length),
            bottom: s
                .absolute
                .and_then(|a| a.bottom)
                .map_or(LengthPercentageAuto::AUTO, LengthPercentageAuto::length),
        },
        ..Default::default()
    }
}

/// Fill paint for boxes and text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fill {
    /// Fill color.
    pub color: Oklch,
}

/// Box border: uniform width + color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Border {
    /// Width in logical px.
    pub width: f32,
    /// Border color.
    pub color: Oklch,
}

/// Drop shadow (outer only in this cut; inset shadows are `[UNIMPLEMENTED]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    /// Horizontal offset.
    pub dx: f32,
    /// Vertical offset.
    pub dy: f32,
    /// Blur radius.
    pub blur: f32,
    /// Shadow color.
    pub color: Oklch,
}

/// Paint properties shared by box-like nodes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Paint {
    /// Background fill (`None` = transparent).
    pub fill: Option<Oklch>,
    /// Uniform corner radius.
    pub radius: f32,
    /// Per-corner radii override (`None` = uniform `radius`).
    pub corners: Option<CornerRadii>,
    /// Border (`None` = no border).
    pub border: Option<Border>,
    /// Drop shadow (`None` = no shadow).
    pub shadow: Option<Shadow>,
    /// Clip children to this node's bounds (rounded by `radius`).
    pub clip: bool,
    /// Group opacity multiplier 0..=1 (`None` = opaque).
    pub opacity: Option<f32>,
}

/// Length helpers: `const` constructors so styles compose in `const`
/// contexts. `px` is fixed logical pixels, `pct` is 0..100+ percent of
/// the parent axis, `auto` is content-sized (unbounded for max sizes).
#[must_use]
pub const fn px(v: f32) -> Length {
    Length::Px(v)
}

/// Percent helper: `pct(50.0)` = 50% of the parent axis. See [`px`].
#[must_use]
pub const fn pct(p: f32) -> Length {
    Length::Pct(p)
}

/// Content-sized helper. See [`px`].
#[must_use]
pub const fn auto() -> Length {
    Length::Auto
}

// Note: `From<f32/f64/i32/u32/usize> for Length` live in `element.rs`
// (element agent) for the *element builders*. `Style` setters below take
// exact `Length` so unit meaning is never ambiguous, and stay `const fn`.
// There are deliberately NO `From<Length> for Insets/f32` conversions:
// units must not silently change meaning (a percent is not pixels).

/// Extract px from a `Length`, const-compatible. Only `Px` is accepted:
/// `Pct`/`Auto` panic (at run time, or at compile time inside `const`
/// styles) instead of silently changing unit meaning. Per-edge values
/// go through [`Style::padding_insets`]/[`Style::margin_insets`].
const fn px_only(len: Length) -> f32 {
    match len {
        Length::Px(v) => v,
        Length::Pct(_) => {
            panic!("Style::padding/margin/radius accept px() only; Pct has no referent here")
        }
        Length::Auto => {
            panic!("Style::padding/margin/radius accept px() only; Auto has no meaning here")
        }
    }
}

/// Resolved typography metadata (mirrors tree `TextContent` text fields).
/// Defaults match `TextContent::label`: 14px, white, no wrap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Typography {
    /// Font size in logical px.
    pub font_size: f32,
    /// Text color.
    pub color: Oklch,
    /// Whether static labels wrap at the available width.
    pub wrap: bool,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            color: Oklch::WHITE,
            wrap: false,
        }
    }
}

/// Canonical partial style: every field starts **unset** and only
/// explicitly set fields affect [`Style::apply_to`] / resolution.
/// Later styles override only set fields, in order; `clear_*` setters
/// record **explicit none** (actively clears the target) as distinct
/// from unset (leaves the target alone).
///
/// All fields are private. Scalar styling never allocates; only the two
/// grid-track setters clone into a `Vec` (documented, non-`const`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    width: Option<Length>,
    height: Option<Length>,
    min_width: Option<Length>,
    min_height: Option<Length>,
    max_width: Option<Length>,
    max_height: Option<Length>,
    flex_grow: Option<f32>,
    flex_shrink: Option<f32>,
    axis: Option<Axis>,
    flex_wrap: Option<bool>,
    align: Option<AlignItems>,
    align_self: Option<Option<AlignSelf>>,
    justify: Option<JustifyContent>,
    gap: Option<f32>,
    column_gap: Option<Option<f32>>,
    row_gap: Option<Option<f32>>,
    padding: Option<Insets>,
    margin: Option<Insets>,
    grid_columns: Option<Option<Vec<Length>>>,
    grid_rows: Option<Option<Vec<Length>>>,
    absolute: Option<Option<Absolute>>,
    fill: Option<Option<Oklch>>,
    radius: Option<f32>,
    corners: Option<Option<CornerRadii>>,
    border: Option<Option<Border>>,
    shadow: Option<Option<Shadow>>,
    clip: Option<bool>,
    opacity: Option<Option<f32>>,
    font_size: Option<f32>,
    text_color: Option<Oklch>,
    text_wrap: Option<bool>,
}

impl Style {
    /// Empty style: sets nothing, changes nothing on apply.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            width: None,
            height: None,
            min_width: None,
            min_height: None,
            max_width: None,
            max_height: None,
            flex_grow: None,
            flex_shrink: None,
            axis: None,
            flex_wrap: None,
            align: None,
            align_self: None,
            justify: None,
            gap: None,
            column_gap: None,
            row_gap: None,
            padding: None,
            margin: None,
            grid_columns: None,
            grid_rows: None,
            absolute: None,
            fill: None,
            radius: None,
            corners: None,
            border: None,
            shadow: None,
            clip: None,
            opacity: None,
            font_size: None,
            text_color: None,
            text_wrap: None,
        }
    }

    /// Whether no field is set (apply is a no-op).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.width.is_none()
            && self.height.is_none()
            && self.min_width.is_none()
            && self.min_height.is_none()
            && self.max_width.is_none()
            && self.max_height.is_none()
            && self.flex_grow.is_none()
            && self.flex_shrink.is_none()
            && self.axis.is_none()
            && self.flex_wrap.is_none()
            && self.align.is_none()
            && self.align_self.is_none()
            && self.justify.is_none()
            && self.gap.is_none()
            && self.column_gap.is_none()
            && self.row_gap.is_none()
            && self.padding.is_none()
            && self.margin.is_none()
            && self.grid_columns.is_none()
            && self.grid_rows.is_none()
            && self.absolute.is_none()
            && self.fill.is_none()
            && self.radius.is_none()
            && self.corners.is_none()
            && self.border.is_none()
            && self.shadow.is_none()
            && self.clip.is_none()
            && self.opacity.is_none()
            && self.font_size.is_none()
            && self.text_color.is_none()
            && self.text_wrap.is_none()
    }

    // -- sizing (exact Length, const) --

    /// Width: `px` | `pct` | `auto`. Exact `Length` — units never convert.
    #[must_use]
    pub const fn width(mut self, w: Length) -> Self {
        self.width = Some(w);
        self
    }

    /// Height. See [`Style::width`].
    #[must_use]
    pub const fn height(mut self, h: Length) -> Self {
        self.height = Some(h);
        self
    }

    /// Minimum width. See [`Style::width`].
    #[must_use]
    pub const fn min_width(mut self, w: Length) -> Self {
        self.min_width = Some(w);
        self
    }

    /// Minimum height. See [`Style::width`].
    #[must_use]
    pub const fn min_height(mut self, h: Length) -> Self {
        self.min_height = Some(h);
        self
    }

    /// Maximum width (`auto` = unbounded). See [`Style::width`].
    #[must_use]
    pub const fn max_width(mut self, w: Length) -> Self {
        self.max_width = Some(w);
        self
    }

    /// Maximum height (`auto` = unbounded). See [`Style::width`].
    #[must_use]
    pub const fn max_height(mut self, h: Length) -> Self {
        self.max_height = Some(h);
        self
    }

    // -- flex --

    /// Flex grow factor.
    #[must_use]
    pub const fn flex_grow(mut self, g: f32) -> Self {
        self.flex_grow = Some(g);
        self
    }

    /// Flex shrink factor.
    #[must_use]
    pub const fn flex_shrink(mut self, s: f32) -> Self {
        self.flex_shrink = Some(s);
        self
    }

    /// Main-axis direction.
    #[must_use]
    pub const fn axis(mut self, axis: Axis) -> Self {
        self.axis = Some(axis);
        self
    }

    /// Horizontal-row container.
    #[must_use]
    pub const fn row(mut self) -> Self {
        self.axis = Some(Axis::Row);
        self
    }

    /// Vertical-column container.
    #[must_use]
    pub const fn column(mut self) -> Self {
        self.axis = Some(Axis::Column);
        self
    }

    /// Whether children wrap (flex container flag; text uses
    /// [`Style::text_wrap`]).
    #[must_use]
    pub const fn wrap(mut self, wrap: bool) -> Self {
        self.flex_wrap = Some(wrap);
        self
    }

    /// Cross-axis alignment of children.
    #[must_use]
    pub const fn align(mut self, align: AlignItems) -> Self {
        self.align = Some(align);
        self
    }

    /// Self-alignment override.
    #[must_use]
    pub const fn align_self(mut self, align: AlignSelf) -> Self {
        self.align_self = Some(Some(align));
        self
    }

    /// Clear a self-alignment override (explicit none: resolves back to
    /// container inheritance).
    #[must_use]
    pub const fn clear_align_self(mut self) -> Self {
        self.align_self = Some(None);
        self
    }

    /// Main-axis distribution.
    #[must_use]
    pub const fn justify(mut self, justify: JustifyContent) -> Self {
        self.justify = Some(justify);
        self
    }

    /// Gap between children on both axes unless overridden.
    #[must_use]
    pub const fn gap(mut self, gap: f32) -> Self {
        self.gap = Some(gap);
        self
    }

    /// Column gap override.
    #[must_use]
    pub const fn column_gap(mut self, gap: f32) -> Self {
        self.column_gap = Some(Some(gap));
        self
    }

    /// Clear the column gap override (falls back to `gap`).
    #[must_use]
    pub const fn clear_column_gap(mut self) -> Self {
        self.column_gap = Some(None);
        self
    }

    /// Row gap override.
    #[must_use]
    pub const fn row_gap(mut self, gap: f32) -> Self {
        self.row_gap = Some(Some(gap));
        self
    }

    /// Clear the row gap override (falls back to `gap`).
    #[must_use]
    pub const fn clear_row_gap(mut self) -> Self {
        self.row_gap = Some(None);
        self
    }

    // -- spacing (uniform px Length, or explicit Insets; const) --

    /// Uniform inner spacing in px. `Pct`/`Auto` are rejected (panic)
    /// rather than silently reinterpreted; per-edge values go through
    /// [`Style::padding_insets`].
    #[must_use]
    pub const fn padding(mut self, pad: Length) -> Self {
        self.padding = Some(Insets::uniform(px_only(pad)));
        self
    }

    /// Per-edge inner spacing.
    #[must_use]
    pub const fn padding_insets(mut self, pad: Insets) -> Self {
        self.padding = Some(pad);
        self
    }

    /// Uniform outer spacing in px. See [`Style::padding`].
    #[must_use]
    pub const fn margin(mut self, m: Length) -> Self {
        self.margin = Some(Insets::uniform(px_only(m)));
        self
    }

    /// Per-edge outer spacing.
    #[must_use]
    pub const fn margin_insets(mut self, m: Insets) -> Self {
        self.margin = Some(m);
        self
    }

    // -- grid (allocating: not const) --

    /// Grid column tracks (owning). `None` (via [`Style::clear_grid_columns`])
    /// selects flex layout.
    #[must_use]
    pub fn grid_columns(mut self, tracks: Vec<Length>) -> Self {
        self.grid_columns = Some(Some(tracks));
        self
    }

    /// Grid column tracks from a slice (clones; the only allocation).
    #[must_use]
    pub fn grid_columns_slice(mut self, tracks: &[Length]) -> Self {
        self.grid_columns = Some(Some(tracks.to_vec()));
        self
    }

    /// Clear grid columns (explicit none: back to flex layout).
    #[must_use]
    pub fn clear_grid_columns(mut self) -> Self {
        self.grid_columns = Some(None);
        self
    }

    /// Grid row tracks (owning).
    #[must_use]
    pub fn grid_rows(mut self, tracks: Vec<Length>) -> Self {
        self.grid_rows = Some(Some(tracks));
        self
    }

    /// Grid row tracks from a slice (clones).
    #[must_use]
    pub fn grid_rows_slice(mut self, tracks: &[Length]) -> Self {
        self.grid_rows = Some(Some(tracks.to_vec()));
        self
    }

    /// Clear grid rows (explicit none).
    #[must_use]
    pub fn clear_grid_rows(mut self) -> Self {
        self.grid_rows = Some(None);
        self
    }

    /// Set both grid track lists from slices.
    #[must_use]
    pub fn grid(mut self, columns: &[Length], rows: &[Length]) -> Self {
        self.grid_columns = Some(Some(columns.to_vec()));
        self.grid_rows = Some(Some(rows.to_vec()));
        self
    }

    // -- positioning --

    /// Absolute positioning (removed from flow).
    #[must_use]
    pub const fn absolute(mut self, absolute: Absolute) -> Self {
        self.absolute = Some(Some(absolute));
        self
    }

    /// Absolute offsets from the padding box (`None` per side = unset).
    #[must_use]
    pub const fn absolute_at(
        mut self,
        left: Option<f32>,
        top: Option<f32>,
        right: Option<f32>,
        bottom: Option<f32>,
    ) -> Self {
        self.absolute = Some(Some(Absolute {
            left,
            top,
            right,
            bottom,
        }));
        self
    }

    /// Clear absolute positioning (explicit none: back in flow).
    #[must_use]
    pub const fn clear_absolute(mut self) -> Self {
        self.absolute = Some(None);
        self
    }

    // -- paint --

    /// Background fill.
    #[must_use]
    pub const fn background(mut self, color: Oklch) -> Self {
        self.fill = Some(Some(color));
        self
    }

    /// Clear the background (explicit none: transparent).
    #[must_use]
    pub const fn clear_background(mut self) -> Self {
        self.fill = Some(None);
        self
    }

    /// Uniform corner radius in px. `Pct`/`Auto` are rejected (panic);
    /// per-corner values go through [`Style::corners`].
    #[must_use]
    pub const fn radius(mut self, r: Length) -> Self {
        self.radius = Some(px_only(r));
        self
    }

    /// Per-corner radii override.
    #[must_use]
    pub const fn corners(mut self, corners: CornerRadii) -> Self {
        self.corners = Some(Some(corners));
        self
    }

    /// Clear per-corner override (falls back to uniform `radius`).
    #[must_use]
    pub const fn clear_corners(mut self) -> Self {
        self.corners = Some(None);
        self
    }

    /// Uniform border.
    #[must_use]
    pub const fn border(mut self, border: Border) -> Self {
        self.border = Some(Some(border));
        self
    }

    /// Border from width + color.
    #[must_use]
    pub const fn border_of(mut self, width: f32, color: Oklch) -> Self {
        self.border = Some(Some(Border { width, color }));
        self
    }

    /// Clear the border (explicit none).
    #[must_use]
    pub const fn clear_border(mut self) -> Self {
        self.border = Some(None);
        self
    }

    /// Drop shadow.
    #[must_use]
    pub const fn shadow(mut self, shadow: Shadow) -> Self {
        self.shadow = Some(Some(shadow));
        self
    }

    /// Shadow from parts.
    #[must_use]
    pub const fn shadow_of(mut self, dx: f32, dy: f32, blur: f32, color: Oklch) -> Self {
        self.shadow = Some(Some(Shadow {
            dx,
            dy,
            blur,
            color,
        }));
        self
    }

    /// Clear the shadow (explicit none).
    #[must_use]
    pub const fn clear_shadow(mut self) -> Self {
        self.shadow = Some(None);
        self
    }

    /// Clip children to bounds.
    #[must_use]
    pub const fn clip(mut self, clip: bool) -> Self {
        self.clip = Some(clip);
        self
    }

    /// Group opacity multiplier 0..=1.
    #[must_use]
    pub const fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(Some(opacity));
        self
    }

    /// Clear opacity (explicit none: opaque).
    #[must_use]
    pub const fn clear_opacity(mut self) -> Self {
        self.opacity = Some(None);
        self
    }

    // -- typography --

    /// Font size in logical px.
    #[must_use]
    pub const fn font_size(mut self, size: f32) -> Self {
        self.font_size = Some(size);
        self
    }

    /// Text color.
    #[must_use]
    pub const fn text_color(mut self, color: Oklch) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Static-label wrapping.
    #[must_use]
    pub const fn text_wrap(mut self, wrap: bool) -> Self {
        self.text_wrap = Some(wrap);
        self
    }

    /// Set font size, if any.
    #[must_use]
    pub fn font_size_opt(&self) -> Option<f32> {
        self.font_size
    }

    /// Set text color, if any.
    #[must_use]
    pub fn text_color_opt(&self) -> Option<Oklch> {
        self.text_color
    }

    /// Set text wrap flag, if any.
    #[must_use]
    pub fn text_wrap_opt(&self) -> Option<bool> {
        self.text_wrap
    }

    // -- merge --

    /// Absorb `other`: every explicitly set field (including explicit
    /// none) overwrites `self`; unset fields leave `self` alone.
    pub fn merge(&mut self, other: &Self) {
        macro_rules! take {
            ($f:ident) => {
                if other.$f.is_some() {
                    self.$f = other.$f.clone();
                }
            };
        }
        take!(width);
        take!(height);
        take!(min_width);
        take!(min_height);
        take!(max_width);
        take!(max_height);
        take!(flex_grow);
        take!(flex_shrink);
        take!(axis);
        take!(flex_wrap);
        take!(align);
        take!(align_self);
        take!(justify);
        take!(gap);
        take!(column_gap);
        take!(row_gap);
        take!(padding);
        take!(margin);
        take!(grid_columns);
        take!(grid_rows);
        take!(absolute);
        take!(fill);
        take!(radius);
        take!(corners);
        take!(border);
        take!(shadow);
        take!(clip);
        take!(opacity);
        take!(font_size);
        take!(text_color);
        take!(text_wrap);
    }

    /// Builder form of [`Style::merge`].
    #[must_use]
    pub fn with(mut self, other: &Self) -> Self {
        self.merge(other);
        self
    }

    /// Fold `styles` in order into one style (later wins per field).
    /// Fixed arrays work: `Style::compose(&[a, b, c])`.
    #[must_use]
    pub fn compose(styles: &[Style]) -> Style {
        let mut out = Style::new();
        for s in styles {
            out.merge(s);
        }
        out
    }

    // -- resolution --

    /// Apply set fields onto an existing resolved pair, leaving the rest
    /// untouched. Preferred tree-integration entry point: resolve once,
    /// no wrapper nodes.
    pub fn apply_to(&self, layout: &mut LayoutStyle, paint: &mut Paint) {
        if let Some(v) = self.width {
            layout.width = v;
        }
        if let Some(v) = self.height {
            layout.height = v;
        }
        if let Some(v) = self.min_width {
            layout.min_width = v;
        }
        if let Some(v) = self.min_height {
            layout.min_height = v;
        }
        if let Some(v) = self.max_width {
            layout.max_width = v;
        }
        if let Some(v) = self.max_height {
            layout.max_height = v;
        }
        if let Some(v) = self.flex_grow {
            layout.flex_grow = v;
        }
        if let Some(v) = self.flex_shrink {
            layout.flex_shrink = v;
        }
        if let Some(v) = self.axis {
            layout.axis = v;
        }
        if let Some(v) = self.flex_wrap {
            layout.wrap = v;
        }
        if let Some(v) = self.align {
            layout.align = v;
        }
        if let Some(v) = self.align_self {
            layout.align_self = v;
        }
        if let Some(v) = self.justify {
            layout.justify = v;
        }
        if let Some(v) = self.gap {
            layout.gap = v;
        }
        if let Some(v) = self.column_gap {
            layout.column_gap = v;
        }
        if let Some(v) = self.row_gap {
            layout.row_gap = v;
        }
        if let Some(v) = self.padding {
            layout.padding = v;
        }
        if let Some(v) = self.margin {
            layout.margin = v;
        }
        if let Some(v) = self.grid_columns.clone() {
            layout.grid_columns = v;
        }
        if let Some(v) = self.grid_rows.clone() {
            layout.grid_rows = v;
        }
        if let Some(v) = self.absolute {
            layout.absolute = v;
        }
        if let Some(v) = self.fill {
            paint.fill = v;
        }
        if let Some(v) = self.radius {
            paint.radius = v;
        }
        if let Some(v) = self.corners {
            paint.corners = v;
        }
        if let Some(v) = self.border {
            paint.border = v;
        }
        if let Some(v) = self.shadow {
            paint.shadow = v;
        }
        if let Some(v) = self.clip {
            paint.clip = v;
        }
        if let Some(v) = self.opacity {
            paint.opacity = v;
        }
    }

    /// Resolve layout against `LayoutStyle::default()`.
    #[must_use]
    pub fn resolve_layout(&self) -> LayoutStyle {
        let mut layout = LayoutStyle::default();
        let mut paint = Paint::default();
        self.apply_to(&mut layout, &mut paint);
        layout
    }

    /// Resolve paint against `Paint::default()`.
    #[must_use]
    pub fn resolve_paint(&self) -> Paint {
        let mut layout = LayoutStyle::default();
        let mut paint = Paint::default();
        self.apply_to(&mut layout, &mut paint);
        paint
    }

    /// Resolve typography against `Typography::default()`.
    #[must_use]
    pub fn resolve_typography(&self) -> Typography {
        Typography {
            font_size: self.font_size.unwrap_or(14.0),
            color: self.text_color.unwrap_or(Oklch::WHITE),
            wrap: self.text_wrap.unwrap_or(false),
        }
    }

    /// Apply typography fields onto tree `TextContent`-shaped targets.
    pub fn apply_typography(&self, font_size: &mut f32, color: &mut Oklch, wrap: &mut bool) {
        if let Some(v) = self.font_size {
            *font_size = v;
        }
        if let Some(v) = self.text_color {
            *color = v;
        }
        if let Some(v) = self.text_wrap {
            *wrap = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flex_row_maps_to_taffy() {
        let s = LayoutStyle {
            axis: Axis::Row,
            gap: 8.0,
            width: Length::Px(100.0),
            ..Default::default()
        };
        let t = to_taffy(&s);
        assert_eq!(t.display, Display::Flex);
        assert_eq!(t.flex_direction, FlexDirection::Row);
        assert_eq!(t.size.width, taffy::style::Dimension::length(100.0));
    }

    #[test]
    fn grid_tracks_map() {
        let s = LayoutStyle {
            grid_columns: Some(vec![Length::Px(40.0), Length::Auto]),
            ..Default::default()
        };
        let t = to_taffy(&s);
        assert_eq!(t.display, taffy::style::Display::Grid);
        assert_eq!(t.grid_template_columns.len(), 2);
    }

    #[test]
    fn length_conversions() {
        assert_eq!(Length::Auto.to_dimension(), taffy::style::Dimension::AUTO);
        assert_eq!(
            Length::Px(4.0).to_dimension(),
            taffy::style::Dimension::length(4.0)
        );
    }

    #[test]
    fn helpers_are_const() {
        const W: Length = px(120.0);
        const P: Length = pct(50.0);
        const A: Length = auto();
        assert_eq!(W, Length::Px(120.0));
        assert_eq!(P, Length::Pct(50.0));
        assert_eq!(A, Length::Auto);
        // No lossy global conversions exist: units never silently change
        // meaning. px-only setters resolve through px_only instead.
        assert_eq!(px_only(px(8.0)), 8.0);
    }

    #[test]
    fn px_only_rejects_pct_and_auto() {
        let before = Style::new().width(px(1.0)).resolve_layout();
        assert_eq!(before.width, Length::Px(1.0));
    }

    #[test]
    #[should_panic(expected = "px() only")]
    fn padding_rejects_pct() {
        let _ = Style::new().padding(pct(10.0));
    }

    #[test]
    #[should_panic(expected = "px() only")]
    fn padding_rejects_auto() {
        let _ = Style::new().padding(auto());
    }

    #[test]
    #[should_panic(expected = "px() only")]
    fn margin_rejects_pct() {
        let _ = Style::new().margin(pct(10.0));
    }

    #[test]
    #[should_panic(expected = "px() only")]
    fn radius_rejects_pct() {
        let _ = Style::new().radius(pct(10.0));
    }

    #[test]
    #[should_panic(expected = "px() only")]
    fn radius_rejects_auto() {
        let _ = Style::new().radius(auto());
    }

    #[test]
    fn per_edge_insets_stay_exact() {
        let s = Style::new()
            .padding_insets(Insets::hv(12.0, 4.0))
            .margin_insets(Insets::uniform(2.0));
        let layout = s.resolve_layout();
        assert_eq!(layout.padding, Insets::hv(12.0, 4.0));
        assert_eq!(layout.margin, Insets::uniform(2.0));
    }

    #[test]
    fn empty_style_changes_nothing() {
        assert!(Style::new().is_empty());
        assert_eq!(Style::new(), Style::default());
        let mut layout = LayoutStyle {
            width: Length::Px(33.0),
            ..Default::default()
        };
        let mut paint = Paint {
            radius: 7.0,
            ..Default::default()
        };
        Style::new().apply_to(&mut layout, &mut paint);
        assert_eq!(layout.width, Length::Px(33.0));
        assert_eq!(paint.radius, 7.0);
    }

    #[test]
    fn const_style_builds_in_const_context() {
        // Every scalar setter is const (grid Vec setters excepted).
        const S: Style = Style::new().clip(true).flex_grow(1.0).gap(8.0);
        assert!(!S.is_empty());
        let paint = S.resolve_paint();
        assert!(paint.clip);
        let layout = S.resolve_layout();
        assert_eq!(layout.flex_grow, 1.0);
        assert_eq!(layout.gap, 8.0);
    }

    #[test]
    fn full_const_chain_with_oklch_new() {
        // The agreed scalar example, frozen as a compile-time style.
        const S: Style = Style::new()
            .width(px(120.0))
            .height(px(80.0))
            .padding(px(8.0))
            .radius(px(6.0))
            .background(Oklch::new(0.6, 0.1, 250.0))
            .text_color(Oklch::WHITE)
            .font_size(14.0)
            .clip(true);
        assert!(!S.is_empty());
        let layout = S.resolve_layout();
        assert_eq!(layout.width, Length::Px(120.0));
        assert_eq!(layout.height, Length::Px(80.0));
        assert_eq!(layout.padding, Insets::uniform(8.0));
        let paint = S.resolve_paint();
        assert_eq!(paint.radius, 6.0);
        assert_eq!(paint.fill, Some(Oklch::new(0.6, 0.1, 250.0)));
        assert!(paint.clip);
        let typo = S.resolve_typography();
        assert_eq!(typo.font_size, 14.0);
        assert_eq!(typo.color, Oklch::WHITE);
    }

    #[test]
    fn required_chain_resolves() {
        let s = Style::new()
            .width(px(120.0))
            .height(pct(50.0))
            .padding(px(8.0))
            .radius(px(6.0))
            .background(Oklch::WHITE)
            .text_color(Oklch::BLACK)
            .font_size(14.0)
            .text_wrap(true);
        let layout = s.resolve_layout();
        assert_eq!(layout.width, Length::Px(120.0));
        assert_eq!(layout.height, Length::Pct(50.0));
        assert_eq!(layout.padding, Insets::uniform(8.0));
        let paint = s.resolve_paint();
        assert_eq!(paint.radius, 6.0);
        assert_eq!(paint.fill, Some(Oklch::WHITE));
        let typo = s.resolve_typography();
        assert_eq!(typo.font_size, 14.0);
        assert_eq!(typo.color, Oklch::BLACK);
        assert!(typo.wrap);
        assert_eq!(s.font_size_opt(), Some(14.0));
        assert_eq!(s.text_color_opt(), Some(Oklch::BLACK));
        assert_eq!(s.text_wrap_opt(), Some(true));
    }

    #[test]
    fn later_partial_overrides_only_set_fields() {
        let base = Style::new()
            .width(px(100.0))
            .gap(4.0)
            .background(Oklch::WHITE);
        let over = Style::new().gap(16.0).text_color(Oklch::BLACK);
        let composed = Style::compose(&[base, over]);
        let layout = composed.resolve_layout();
        // Untouched by `over`: still base values.
        assert_eq!(layout.width, Length::Px(100.0));
        // Set by `over`: wins.
        assert_eq!(layout.gap, 16.0);
        assert_eq!(composed.resolve_paint().fill, Some(Oklch::WHITE));
        assert_eq!(composed.text_color_opt(), Some(Oklch::BLACK));
    }

    #[test]
    fn explicit_none_clears_optional_effects() {
        // Set then clear: resolves to None, not to the earlier value.
        let s = Style::new()
            .background(Oklch::WHITE)
            .clear_background()
            .border_of(2.0, Oklch::BLACK)
            .clear_border()
            .shadow_of(1.0, 2.0, 3.0, Oklch::BLACK)
            .clear_shadow()
            .opacity(0.5)
            .clear_opacity()
            .corners(CornerRadii::uniform(4.0))
            .clear_corners()
            .align_self(AlignSelf::CENTER)
            .clear_align_self()
            .column_gap(9.0)
            .clear_column_gap()
            .row_gap(9.0)
            .clear_row_gap()
            .grid_columns_slice(&[px(40.0)])
            .clear_grid_columns()
            .grid_rows_slice(&[px(20.0)])
            .clear_grid_rows()
            .absolute_at(Some(1.0), None, None, None)
            .clear_absolute();
        let paint = s.resolve_paint();
        assert_eq!(paint.fill, None);
        assert_eq!(paint.border, None);
        assert_eq!(paint.shadow, None);
        assert_eq!(paint.opacity, None);
        assert_eq!(paint.corners, None);
        let layout = s.resolve_layout();
        assert_eq!(layout.align_self, None);
        assert_eq!(layout.column_gap, None);
        assert_eq!(layout.row_gap, None);
        assert_eq!(layout.grid_columns, None);
        assert_eq!(layout.grid_rows, None);
        assert_eq!(layout.absolute, None);

        // Merge order: later explicit-none beats earlier value.
        let a = Style::new().background(Oklch::WHITE);
        let b = Style::new().clear_background();
        assert_eq!(
            Style::compose(&[a.clone(), b.clone()]).resolve_paint().fill,
            None
        );
        // And a later value beats an earlier clear.
        let c = Style::new().background(Oklch::BLACK);
        assert_eq!(
            Style::compose(&[a, b, c]).resolve_paint().fill,
            Some(Oklch::BLACK)
        );
    }

    #[test]
    fn grid_slice_clones_and_applies() {
        let tracks = [px(40.0), auto()];
        let s = Style::new().grid_columns_slice(&tracks);
        // Caller array is untouched (slice was cloned, not borrowed).
        assert_eq!(tracks, [Length::Px(40.0), Length::Auto]);
        let layout = s.resolve_layout();
        assert_eq!(
            layout.grid_columns,
            Some(vec![Length::Px(40.0), Length::Auto])
        );
        let t = to_taffy(&layout);
        assert_eq!(t.display, taffy::style::Display::Grid);
        assert_eq!(t.grid_template_columns.len(), 2);
    }

    #[test]
    fn layout_end_to_end_through_taffy() {
        let s = Style::new()
            .width(px(100.0))
            .height(px(50.0))
            .column()
            .gap(8.0)
            .padding_insets(Insets::hv(12.0, 4.0));
        let t = to_taffy(&s.resolve_layout());
        assert_eq!(t.flex_direction, FlexDirection::Column);
        assert_eq!(t.size.width, taffy::style::Dimension::length(100.0));
        assert_eq!(t.size.height, taffy::style::Dimension::length(50.0));
    }

    #[test]
    fn scalar_styles_leave_grid_storage_unset() {
        // Scalar styles must not introduce grid definitions.
        // This checks resolved state, not allocator activity.
        let s = Style::compose(&[
            Style::new().width(px(10.0)).clip(true),
            Style::new().background(Oklch::WHITE).font_size(12.0),
        ]);
        let layout = s.resolve_layout();
        assert!(layout.grid_columns.is_none());
        assert!(layout.grid_rows.is_none());
    }
}
