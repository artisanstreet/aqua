use aqua_core::Insets;
use aqua_style::{Axis, LayoutStyle, Length};
use taffy::prelude::TaffyAuto as _;
use taffy::style::{
    AlignContent, Dimension, Display, FlexDirection, FlexWrap, LengthPercentage,
    LengthPercentageAuto, MaxTrackSizingFunction, MinTrackSizingFunction, Position,
};

fn length(value: Length) -> LengthPercentage {
    match value {
        Length::Px(v) => LengthPercentage::length(v),
        Length::Pct(p) => LengthPercentage::percent(p / 100.0),
        Length::Auto => LengthPercentage::length(0.0),
    }
}

pub fn dimension(value: Length) -> Dimension {
    match value {
        Length::Px(v) => Dimension::length(v),
        Length::Pct(p) => Dimension::percent(p / 100.0),
        Length::Auto => Dimension::AUTO,
    }
}

fn auto_length(value: Length) -> LengthPercentageAuto {
    match value {
        Length::Px(v) => LengthPercentageAuto::length(v),
        Length::Pct(p) => LengthPercentageAuto::percent(p / 100.0),
        Length::Auto => LengthPercentageAuto::AUTO,
    }
}

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
    let grid_style = (s.grid_columns.is_some() || s.grid_rows.is_some()).then(|| {
        Box::new(taffy::style::GridStyle {
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
            ..Default::default()
        })
    });
    taffy::Style {
        display,
        size: taffy::geometry::Size {
            width: dimension(s.width),
            height: dimension(s.height),
        },
        min_size: taffy::geometry::Size {
            width: auto_length(s.min_width),
            height: auto_length(s.min_height),
        },
        max_size: taffy::geometry::Size {
            width: auto_length(s.max_width),
            height: auto_length(s.max_height),
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
            width: length(Length::Px(s.column_gap.unwrap_or(s.gap))),
            height: length(Length::Px(s.row_gap.unwrap_or(s.gap))),
        },
        padding: rect(s.padding),
        margin: taffy::geometry::Rect {
            left: LengthPercentageAuto::length(s.margin.left),
            right: LengthPercentageAuto::length(s.margin.right),
            top: LengthPercentageAuto::length(s.margin.top),
            bottom: LengthPercentageAuto::length(s.margin.bottom),
        },
        grid: grid_style,
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
