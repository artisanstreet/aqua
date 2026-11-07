use aqua_core::{DisplayItem, Rect};
use aqua_motion::{Edge, MotionValue, Property};

#[must_use]
pub fn apply(item: &mut DisplayItem, property: Property, value: MotionValue) -> bool {
    match (property, value) {
        (Property::Opacity, MotionValue::Scalar(opacity)) => set_opacity(item, opacity),
        (Property::Translate, MotionValue::Pair(x, y)) => match item {
            DisplayItem::PushTransform(transform) => {
                transform.tx = x;
                transform.ty = y;
                true
            }
            _ => false,
        },
        (Property::Fill, MotionValue::Color(color)) => item.set_fill(color),
        (Property::BorderColor, MotionValue::Color(color)) => match item {
            DisplayItem::Rect { border_color, .. } | DisplayItem::Quad { border_color, .. } => {
                *border_color = color;
                true
            }
            _ => false,
        },
        (Property::TextColor, MotionValue::Color(color)) => match item {
            DisplayItem::Text { color: text, .. } => {
                *text = color;
                true
            }
            _ => false,
        },
        (Property::Edge(edge), MotionValue::Scalar(position)) => match item {
            DisplayItem::Rect { bounds, .. }
            | DisplayItem::SrgbaRect { bounds, .. }
            | DisplayItem::Quad { bounds, .. }
            | DisplayItem::Shadow { bounds, .. } => {
                *bounds = move_edge(*bounds, edge, position);
                true
            }
            _ => false,
        },
        _ => false,
    }
}

fn set_opacity(item: &mut DisplayItem, value: f32) -> bool {
    match item {
        DisplayItem::PushOpacity(opacity)
        | DisplayItem::Image { opacity, .. }
        | DisplayItem::Vector { opacity, .. }
        | DisplayItem::Text { opacity, .. }
        | DisplayItem::External { opacity, .. } => {
            *opacity = value;
            true
        }
        _ => false,
    }
}

fn move_edge(bounds: Rect, edge: Edge, position: f32) -> Rect {
    let (left, top, right, bottom) = (bounds.x, bounds.y, bounds.right(), bounds.bottom());
    match edge {
        Edge::Left => Rect::from_edges(position, top, right.max(position), bottom),
        Edge::Top => Rect::from_edges(left, position, right, bottom.max(position)),
        Edge::Right => Rect::from_edges(left, top, position.max(left), bottom),
        Edge::Bottom => Rect::from_edges(left, top, right, position.max(top)),
    }
}
