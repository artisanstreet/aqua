#![forbid(unsafe_code)]


pub mod motion;

use aqua_core::{CornerRadii, Insets, Oklch, Transform2D};
use aqua_text::TextStyle;
use taffy::style::{AlignItems, AlignSelf, JustifyContent};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f32),
    Pct(f32),
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Axis {
    #[default]
    Row,
    Column,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutStyle {
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub min_height: Length,
    pub max_width: Length,
    pub max_height: Length,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub axis: Axis,
    pub wrap: bool,
    pub align: AlignItems,
    pub align_self: Option<AlignSelf>,
    pub justify: JustifyContent,
    pub gap: f32,
    pub column_gap: Option<f32>,
    pub row_gap: Option<f32>,
    pub padding: Insets,
    pub margin: Insets,
    pub grid_columns: Option<Vec<Length>>,
    pub grid_rows: Option<Vec<Length>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Absolute {
    pub left: Option<f32>,
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fill {
    pub color: Oklch,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Border {
    pub width: f32,
    pub color: Oklch,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub color: Oklch,
}

pub use aqua_core::scene::{GpuiShadowLayer, ShadowLayer};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuiBorder {
    pub widths: Insets,
    pub color: aqua_core::GpuiHsla,
    pub dashed: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paint {
    pub motion: Option<std::rc::Rc<crate::motion::MotionSpec>>,
    pub gpui_quad_pulse: Option<aqua_core::scene::GpuiQuadPulse>,
    pub transform: Option<Transform2D>,
    pub content_blur: Option<f32>,
    pub text_color_map: Option<aqua_core::scene::TextColorMap>,
    pub gpui_text_color_map: Option<aqua_core::scene::GpuiTextColorMap>,
    pub gpui_text_shimmer: Option<aqua_core::scene::GpuiTextShimmer>,
    pub fill: Option<Oklch>,
    pub gpui_background: Option<aqua_core::GpuiBackground>,
    pub radius: f32,
    pub corners: Option<CornerRadii>,
    pub border: Option<Border>,
    pub gpui_border: Option<GpuiBorder>,
    pub border_widths: Option<Insets>,
    pub border_dashed: bool,
    pub focus_ring: Option<Border>,
    pub shadow: Option<Shadow>,
    pub shadows: Vec<ShadowLayer>,
    pub gpui_shadows: Vec<GpuiShadowLayer>,
    pub clip: bool,
    pub opacity: Option<f32>,
}

#[must_use]
pub const fn px(v: f32) -> Length {
    Length::Px(v)
}

#[must_use]
pub const fn pct(p: f32) -> Length {
    Length::Pct(p)
}

#[must_use]
pub const fn auto() -> Length {
    Length::Auto
}


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

#[derive(Debug, Clone, PartialEq)]
pub struct Typography {
    pub text_style: TextStyle,
    pub font_size: f32,
    pub color: Oklch,
    pub wrap: bool,
}

static ROOT_LETTER_SPACING_BITS: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(f32::NAN.to_bits());

pub fn set_root_letter_spacing_em(spacing: Option<f32>) {
    ROOT_LETTER_SPACING_BITS.store(
        spacing.unwrap_or(f32::NAN).to_bits(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

pub fn root_letter_spacing_em() -> Option<f32> {
    let value = f32::from_bits(ROOT_LETTER_SPACING_BITS.load(std::sync::atomic::Ordering::Relaxed));
    (!value.is_nan()).then_some(value)
}

pub fn with_root_letter_spacing(mut style: TextStyle) -> TextStyle {
    if style.letter_spacing_em.is_none() {
        style.letter_spacing_em = root_letter_spacing_em();
    }
    style
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            text_style: with_root_letter_spacing(TextStyle {
                size_px: 14.0,
                line_height_px: 16.8,
                ..Default::default()
            }),
            font_size: 14.0,
            color: Oklch::WHITE,
            wrap: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    gpui_quad_pulse: Option<Option<aqua_core::scene::GpuiQuadPulse>>,
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
    gpui_background: Option<Option<aqua_core::GpuiBackground>>,
    radius: Option<f32>,
    corners: Option<Option<CornerRadii>>,
    border: Option<Option<Border>>,
    gpui_border: Option<Option<GpuiBorder>>,
    border_widths: Option<Option<Insets>>,
    border_dashed: Option<bool>,
    focus_ring: Option<Option<Border>>,
    shadow: Option<Option<Shadow>>,
    shadows: Option<Vec<ShadowLayer>>,
    gpui_shadows: Option<Vec<GpuiShadowLayer>>,
    clip: Option<bool>,
    opacity: Option<Option<f32>>,
    transform: Option<Option<Transform2D>>,
    content_blur: Option<Option<f32>>,
    text_color_map: Option<Option<aqua_core::scene::TextColorMap>>,
    gpui_text_color_map: Option<Option<aqua_core::scene::GpuiTextColorMap>>,
    gpui_text_shimmer: Option<Option<aqua_core::scene::GpuiTextShimmer>>,
    font_size: Option<f32>,
    text_style: Option<TextStyle>,
    text_color: Option<Oklch>,
    gpui_text_color: Option<Option<aqua_core::GpuiHsla>>,
    gpui_source_text_color: Option<Option<aqua_core::GpuiTextColor>>,
    text_wrap: Option<bool>,
}

impl Style {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gpui_quad_pulse: None,
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
            gpui_background: None,
            radius: None,
            corners: None,
            border: None,
            gpui_border: None,
            border_widths: None,
            border_dashed: None,
            focus_ring: None,
            shadow: None,
            shadows: None,
            gpui_shadows: None,
            clip: None,
            opacity: None,
            transform: None,
            content_blur: None,
            text_color_map: None,
            gpui_text_color_map: None,
            gpui_text_shimmer: None,
            font_size: None,
            text_style: None,
            text_color: None,
            gpui_text_color: None,
            gpui_source_text_color: None,
            text_wrap: None,
        }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.gpui_quad_pulse.is_none()
            && self.width.is_none()
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
            && self.gpui_background.is_none()
            && self.radius.is_none()
            && self.corners.is_none()
            && self.border.is_none()
            && self.gpui_border.is_none()
            && self.border_widths.is_none()
            && self.border_dashed.is_none()
            && self.focus_ring.is_none()
            && self.shadow.is_none()
            && self.shadows.is_none()
            && self.gpui_shadows.is_none()
            && self.clip.is_none()
            && self.opacity.is_none()
            && self.transform.is_none()
            && self.content_blur.is_none()
            && self.text_color_map.is_none()
            && self.gpui_text_color_map.is_none()
            && self.gpui_text_shimmer.is_none()
            && self.text_style.is_none()
            && self.font_size.is_none()
            && self.text_color.is_none()
            && self.gpui_text_color.is_none()
            && self.gpui_source_text_color.is_none()
            && self.text_wrap.is_none()
    }


    #[must_use]
    pub const fn width(mut self, w: Length) -> Self {
        self.width = Some(w);
        self
    }

    #[must_use]
    pub const fn height(mut self, h: Length) -> Self {
        self.height = Some(h);
        self
    }

    #[must_use]
    pub const fn min_width(mut self, w: Length) -> Self {
        self.min_width = Some(w);
        self
    }

    #[must_use]
    pub const fn min_height(mut self, h: Length) -> Self {
        self.min_height = Some(h);
        self
    }

    #[must_use]
    pub const fn max_width(mut self, w: Length) -> Self {
        self.max_width = Some(w);
        self
    }

    #[must_use]
    pub const fn max_height(mut self, h: Length) -> Self {
        self.max_height = Some(h);
        self
    }


    #[must_use]
    pub const fn flex_grow(mut self, g: f32) -> Self {
        self.flex_grow = Some(g);
        self
    }

    #[must_use]
    pub const fn flex_shrink(mut self, s: f32) -> Self {
        self.flex_shrink = Some(s);
        self
    }

    #[must_use]
    pub const fn axis(mut self, axis: Axis) -> Self {
        self.axis = Some(axis);
        self
    }

    #[must_use]
    pub const fn row(mut self) -> Self {
        self.axis = Some(Axis::Row);
        self
    }

    #[must_use]
    pub const fn column(mut self) -> Self {
        self.axis = Some(Axis::Column);
        self
    }

    #[must_use]
    pub const fn wrap(mut self, wrap: bool) -> Self {
        self.flex_wrap = Some(wrap);
        self
    }

    #[must_use]
    pub const fn align(mut self, align: AlignItems) -> Self {
        self.align = Some(align);
        self
    }

    #[must_use]
    pub const fn align_self(mut self, align: AlignSelf) -> Self {
        self.align_self = Some(Some(align));
        self
    }

    #[must_use]
    pub const fn clear_align_self(mut self) -> Self {
        self.align_self = Some(None);
        self
    }

    #[must_use]
    pub const fn justify(mut self, justify: JustifyContent) -> Self {
        self.justify = Some(justify);
        self
    }

    #[must_use]
    pub const fn gap(mut self, gap: f32) -> Self {
        self.gap = Some(gap);
        self
    }

    #[must_use]
    pub const fn column_gap(mut self, gap: f32) -> Self {
        self.column_gap = Some(Some(gap));
        self
    }

    #[must_use]
    pub const fn clear_column_gap(mut self) -> Self {
        self.column_gap = Some(None);
        self
    }

    #[must_use]
    pub const fn row_gap(mut self, gap: f32) -> Self {
        self.row_gap = Some(Some(gap));
        self
    }

    #[must_use]
    pub const fn clear_row_gap(mut self) -> Self {
        self.row_gap = Some(None);
        self
    }


    #[must_use]
    pub const fn padding(mut self, pad: Length) -> Self {
        self.padding = Some(Insets::uniform(px_only(pad)));
        self
    }

    #[must_use]
    pub const fn padding_insets(mut self, pad: Insets) -> Self {
        self.padding = Some(pad);
        self
    }

    #[must_use]
    pub const fn margin(mut self, m: Length) -> Self {
        self.margin = Some(Insets::uniform(px_only(m)));
        self
    }

    #[must_use]
    pub const fn margin_insets(mut self, m: Insets) -> Self {
        self.margin = Some(m);
        self
    }


    #[must_use]
    pub fn grid_columns(mut self, tracks: Vec<Length>) -> Self {
        self.grid_columns = Some(Some(tracks));
        self
    }

    #[must_use]
    pub fn grid_columns_slice(mut self, tracks: &[Length]) -> Self {
        self.grid_columns = Some(Some(tracks.to_vec()));
        self
    }

    #[must_use]
    pub fn clear_grid_columns(mut self) -> Self {
        self.grid_columns = Some(None);
        self
    }

    #[must_use]
    pub fn grid_rows(mut self, tracks: Vec<Length>) -> Self {
        self.grid_rows = Some(Some(tracks));
        self
    }

    #[must_use]
    pub fn grid_rows_slice(mut self, tracks: &[Length]) -> Self {
        self.grid_rows = Some(Some(tracks.to_vec()));
        self
    }

    #[must_use]
    pub fn clear_grid_rows(mut self) -> Self {
        self.grid_rows = Some(None);
        self
    }

    #[must_use]
    pub fn grid(mut self, columns: &[Length], rows: &[Length]) -> Self {
        self.grid_columns = Some(Some(columns.to_vec()));
        self.grid_rows = Some(Some(rows.to_vec()));
        self
    }


    #[must_use]
    pub const fn absolute(mut self, absolute: Absolute) -> Self {
        self.absolute = Some(Some(absolute));
        self
    }

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

    #[must_use]
    pub const fn clear_absolute(mut self) -> Self {
        self.absolute = Some(None);
        self
    }


    #[must_use]
    pub const fn background(mut self, color: Oklch) -> Self {
        self.gpui_background = Some(None);
        self.fill = Some(Some(color));
        self
    }

    #[must_use]
    pub const fn gpui_background(mut self, background: aqua_core::GpuiBackground) -> Self {
        self.fill = Some(None);
        self.gpui_background = Some(Some(background));
        self
    }

    pub const fn gpui_quad_pulse(mut self, pulse: aqua_core::scene::GpuiQuadPulse) -> Self {
        self.gpui_quad_pulse = Some(Some(pulse));
        self
    }

    #[must_use]
    pub const fn clear_background(mut self) -> Self {
        self.gpui_background = Some(None);
        self.fill = Some(None);
        self
    }

    #[must_use]
    pub const fn radius(mut self, r: Length) -> Self {
        self.radius = Some(px_only(r));
        self
    }

    #[must_use]
    pub const fn corners(mut self, corners: CornerRadii) -> Self {
        self.corners = Some(Some(corners));
        self
    }

    #[must_use]
    pub const fn clear_corners(mut self) -> Self {
        self.corners = Some(None);
        self
    }

    #[must_use]
    pub const fn border(mut self, border: Border) -> Self {
        self.gpui_border = Some(None);
        self.border_widths = Some(None);
        self.border = Some(Some(border));
        self
    }

    #[must_use]
    pub const fn focus_ring(mut self, width: f32, color: Oklch) -> Self {
        self.focus_ring = Some(Some(Border { width, color }));
        self
    }

    pub const fn clear_focus_ring(mut self) -> Self {
        self.focus_ring = Some(None);
        self
    }

    pub const fn border_of(mut self, width: f32, color: Oklch) -> Self {
        self.gpui_border = Some(None);
        self.border_widths = Some(None);
        self.border = Some(Some(Border { width, color }));
        self
    }

    #[must_use]
    pub const fn gpui_border(
        mut self,
        widths: Insets,
        color: aqua_core::GpuiHsla,
        dashed: bool,
    ) -> Self {
        self.border = Some(None);
        self.gpui_border = Some(Some(GpuiBorder {
            widths,
            color,
            dashed,
        }));
        self
    }

    #[must_use]
    pub const fn border_edges(mut self, widths: Insets, color: Oklch) -> Self {
        self.gpui_border = Some(None);
        self.border_widths = Some(Some(widths));
        self.border = Some(Some(Border { width: 0.0, color }));
        self
    }

    #[must_use]
    pub const fn border_dashed(mut self, dashed: bool) -> Self {
        self.border_dashed = Some(dashed);
        self
    }

    #[must_use]
    pub const fn clear_border(mut self) -> Self {
        self.gpui_border = Some(None);
        self.border_widths = Some(None);
        self.border = Some(None);
        self
    }

    #[must_use]
    pub const fn shadow(mut self, shadow: Shadow) -> Self {
        self.shadow = Some(Some(shadow));
        self
    }

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

    #[must_use]
    pub fn shadows(mut self, shadows: Vec<ShadowLayer>) -> Self {
        self.shadow = None;
        self.shadows = Some(shadows);
        self.gpui_shadows = None;
        self
    }

    #[must_use]
    pub fn gpui_shadows(mut self, shadows: Vec<GpuiShadowLayer>) -> Self {
        self.shadow = None;
        self.shadows = None;
        self.gpui_shadows = Some(shadows);
        self
    }

    #[must_use]
    pub const fn clear_shadow(mut self) -> Self {
        self.shadow = Some(None);
        self
    }

    #[must_use]
    pub const fn clip(mut self, clip: bool) -> Self {
        self.clip = Some(clip);
        self
    }

    pub const fn transform(mut self, transform: Transform2D) -> Self {
        self.transform = Some(Some(transform));
        self
    }

    pub const fn clear_transform(mut self) -> Self {
        self.transform = Some(None);
        self
    }

    pub const fn text_color_map(mut self, map: aqua_core::scene::TextColorMap) -> Self {
        self.gpui_text_shimmer = Some(None);
        self.gpui_text_color_map = Some(None);
        self.text_color_map = Some(Some(map));
        self
    }

    #[must_use]
    pub const fn gpui_text_color_map(mut self, map: aqua_core::scene::GpuiTextColorMap) -> Self {
        self.gpui_text_shimmer = Some(None);
        self.text_color_map = Some(None);
        self.gpui_text_color_map = Some(Some(map));
        self
    }

    pub const fn gpui_text_shimmer(mut self, shimmer: aqua_core::scene::GpuiTextShimmer) -> Self {
        self.text_color_map = Some(None);
        self.gpui_text_color_map = Some(None);
        self.gpui_text_shimmer = Some(Some(shimmer));
        self
    }

    pub const fn clear_text_color_map(mut self) -> Self {
        self.gpui_text_shimmer = Some(None);
        self.gpui_text_color_map = Some(None);
        self.text_color_map = Some(None);
        self
    }

    pub const fn content_blur(mut self, radius: f32) -> Self {
        self.content_blur = Some(Some(radius));
        self
    }

    pub const fn clear_content_blur(mut self) -> Self {
        self.content_blur = Some(None);
        self
    }

    #[must_use]
    pub const fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(Some(opacity));
        self
    }

    #[must_use]
    pub const fn clear_opacity(mut self) -> Self {
        self.opacity = Some(None);
        self
    }


    #[must_use]
    pub const fn font_size(mut self, size: f32) -> Self {
        self.font_size = Some(size);
        self
    }

    #[must_use]
    pub const fn text_color(mut self, color: Oklch) -> Self {
        self.text_color = Some(color);
        self.gpui_text_color = Some(None);
        self.gpui_source_text_color = Some(None);
        self
    }

    pub const fn gpui_text_color(mut self, color: aqua_core::GpuiHsla) -> Self {
        self.gpui_text_color = Some(Some(color));
        self.gpui_source_text_color = Some(None);
        self
    }

    pub const fn gpui_source_text_color(mut self, color: aqua_core::GpuiTextColor) -> Self {
        self.gpui_text_color = Some(None);
        self.gpui_source_text_color = Some(Some(color));
        self
    }

    pub const fn gpui_source_text_color_opt(&self) -> Option<Option<aqua_core::GpuiTextColor>> {
        self.gpui_source_text_color
    }

    pub const fn gpui_text_color_opt(&self) -> Option<Option<aqua_core::GpuiHsla>> {
        self.gpui_text_color
    }

    #[must_use]
    pub const fn text_wrap(mut self, wrap: bool) -> Self {
        self.text_wrap = Some(wrap);
        self
    }

    #[must_use]
    pub fn text_style(mut self, style: TextStyle) -> Self {
        self.font_size = Some(style.size_px);
        if let Some([hue_degrees, saturation, lightness, alpha]) = style.gpui_source_color {
            self.gpui_text_color = Some(None);
            self.gpui_source_text_color = Some(Some(aqua_core::GpuiTextColor {
                hue_degrees,
                saturation,
                lightness,
                alpha,
            }));
        } else if let Some([h, s, l, a]) = style.gpui_color {
            self.gpui_text_color = Some(Some(aqua_core::GpuiHsla { h, s, l, a }));
            self.gpui_source_text_color = Some(None);
        }
        self.text_style = Some(style);
        self
    }

    pub fn text_style_opt(&self) -> Option<&TextStyle> {
        self.text_style.as_ref()
    }

    pub fn font_size_opt(&self) -> Option<f32> {
        self.font_size
    }

    #[must_use]
    pub fn text_color_opt(&self) -> Option<Oklch> {
        self.text_color
    }

    #[must_use]
    pub fn text_wrap_opt(&self) -> Option<bool> {
        self.text_wrap
    }


    pub fn merge(&mut self, other: &Self) {
        macro_rules! take {
            ($f:ident) => {
                if other.$f.is_some() {
                    self.$f = other.$f.clone();
                }
            };
        }
        take!(gpui_quad_pulse);
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
        take!(gpui_background);
        take!(radius);
        take!(corners);
        take!(border);
        take!(gpui_border);
        take!(border_widths);
        take!(border_dashed);
        take!(focus_ring);
        if other.shadow.is_some() {
            self.shadow = other.shadow;
            self.shadows = None;
            self.gpui_shadows = None;
        } else if let Some(shadows) = &other.shadows {
            self.shadow = None;
            self.shadows = Some(shadows.clone());
            self.gpui_shadows = None;
        } else if let Some(shadows) = &other.gpui_shadows {
            self.shadow = None;
            self.shadows = None;
            self.gpui_shadows = Some(shadows.clone());
        }
        take!(clip);
        take!(opacity);
        take!(transform);
        take!(content_blur);
        take!(text_color_map);
        take!(gpui_text_color_map);
        take!(gpui_text_shimmer);
        take!(font_size);
        take!(text_style);
        take!(text_color);
        take!(gpui_text_color);
        take!(gpui_source_text_color);
        take!(text_wrap);
    }

    #[must_use]
    pub fn with(mut self, other: &Self) -> Self {
        self.merge(other);
        self
    }

    #[must_use]
    pub fn compose(styles: &[Style]) -> Style {
        let mut out = Style::new();
        for s in styles {
            out.merge(s);
        }
        out
    }


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
        if let Some(v) = self.gpui_quad_pulse {
            paint.gpui_quad_pulse = v;
        }
        if let Some(v) = self.fill {
            paint.fill = v;
        }
        if let Some(v) = self.gpui_background {
            paint.gpui_background = v;
        }
        if let Some(v) = self.radius {
            paint.radius = v;
        }
        if let Some(v) = self.corners {
            paint.corners = v;
        }
        if let Some(v) = self.focus_ring {
            paint.focus_ring = v;
        }
        if let Some(v) = self.border {
            paint.border = v;
        }
        if let Some(v) = self.gpui_border {
            paint.gpui_border = v;
        }
        if let Some(v) = self.border_widths {
            paint.border_widths = v;
        }
        if let Some(v) = self.border_dashed {
            paint.border_dashed = v;
        }
        if let Some(v) = self.shadow {
            paint.shadow = v;
            paint.shadows.clear();
            paint.gpui_shadows.clear();
        } else if let Some(v) = &self.shadows {
            paint.shadow = None;
            paint.shadows = v.clone();
            paint.gpui_shadows.clear();
        } else if let Some(v) = &self.gpui_shadows {
            paint.shadow = None;
            paint.shadows.clear();
            paint.gpui_shadows = v.clone();
        }
        if let Some(v) = self.clip {
            paint.clip = v;
        }
        if let Some(v) = self.transform {
            paint.transform = v;
        }
        if let Some(v) = self.content_blur {
            paint.content_blur = v;
        }
        if let Some(v) = self.text_color_map {
            paint.text_color_map = v;
        }
        if let Some(v) = self.gpui_text_color_map {
            paint.gpui_text_color_map = v;
        }
        if let Some(v) = self.gpui_text_shimmer {
            paint.gpui_text_shimmer = v;
        }
        if let Some(v) = self.opacity {
            paint.opacity = v;
        }
    }

    #[must_use]
    pub fn resolve_layout(&self) -> LayoutStyle {
        let mut layout = LayoutStyle::default();
        let mut paint = Paint::default();
        self.apply_to(&mut layout, &mut paint);
        layout
    }

    #[must_use]
    pub fn resolve_paint(&self) -> Paint {
        let mut layout = LayoutStyle::default();
        let mut paint = Paint::default();
        self.apply_to(&mut layout, &mut paint);
        paint
    }

    #[must_use]
    pub fn resolve_typography(&self) -> Typography {
        let mut text_style = with_root_letter_spacing(
            self.text_style
                .clone()
                .unwrap_or_else(|| Typography::default().text_style),
        );
        if let Some(color) = self.gpui_text_color {
            text_style.gpui_color = color.map(|c| [c.h, c.s, c.l, c.a]);
        }
        if let Some(color) = self.gpui_source_text_color {
            text_style.gpui_source_color =
                color.map(|c| [c.hue_degrees, c.saturation, c.lightness, c.alpha]);
        }
        Typography {
            text_style,
            font_size: self.font_size.unwrap_or(14.0),
            color: self.text_color.unwrap_or(Oklch::WHITE),
            wrap: self.text_wrap.unwrap_or(false),
        }
    }

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

impl From<i32> for Length {
    fn from(v: i32) -> Self {
        Length::Px(v as f32)
    }
}

impl From<u32> for Length {
    fn from(v: u32) -> Self {
        Length::Px(v as f32)
    }
}

impl From<usize> for Length {
    fn from(v: usize) -> Self {
        Length::Px(v as f32)
    }
}

impl From<f32> for Length {
    fn from(v: f32) -> Self {
        Length::Px(v)
    }
}

impl From<f64> for Length {
    fn from(v: f64) -> Self {
        Length::Px(v as f32)
    }
}

impl From<&str> for Axis {
    fn from(v: &str) -> Self {
        match v {
            "row" => Axis::Row,
            "column" | "col" => Axis::Column,
            other => {
                panic!("aqua-ui: unknown layout axis `{other}` (expected \"row\" or \"column\")")
            }
        }
    }
}

impl From<String> for Axis {
    fn from(v: String) -> Self {
        Axis::from(v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_are_const() {
        const W: Length = px(120.0);
        const P: Length = pct(50.0);
        const A: Length = auto();
        assert_eq!(W, Length::Px(120.0));
        assert_eq!(P, Length::Pct(50.0));
        assert_eq!(A, Length::Auto);
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
        assert_eq!(layout.width, Length::Px(100.0));
        assert_eq!(layout.gap, 16.0);
        assert_eq!(composed.resolve_paint().fill, Some(Oklch::WHITE));
        assert_eq!(composed.text_color_opt(), Some(Oklch::BLACK));
    }

    #[test]
    fn explicit_none_clears_optional_effects() {
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

        let a = Style::new().background(Oklch::WHITE);
        let b = Style::new().clear_background();
        assert_eq!(
            Style::compose(&[a.clone(), b.clone()]).resolve_paint().fill,
            None
        );
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
        assert_eq!(tracks, [Length::Px(40.0), Length::Auto]);
        let layout = s.resolve_layout();
        assert_eq!(
            layout.grid_columns,
            Some(vec![Length::Px(40.0), Length::Auto])
        );
    }

    #[test]
    fn scalar_styles_leave_grid_storage_unset() {
        let s = Style::compose(&[
            Style::new().width(px(10.0)).clip(true),
            Style::new().background(Oklch::WHITE).font_size(12.0),
        ]);
        let layout = s.resolve_layout();
        assert!(layout.grid_columns.is_none());
        assert!(layout.grid_rows.is_none());
    }
}
