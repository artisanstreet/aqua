//! 2D geometry in logical units (`f32`). Application layout uses logical
//! coordinates; rendering boundaries convert to physical pixels with an
//! explicit scale factor (see `Transform2D` / the render contract).

/// A point in logical units.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    /// Logical x.
    pub x: f32,
    /// Logical y.
    pub y: f32,
}

impl Point {
    /// The origin.
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    /// Construct a point.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A 2D extent. Width/height are `>= 0` by convention; constructors clamp.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    /// Logical width.
    pub w: f32,
    /// Logical height.
    pub h: f32,
}

impl Size {
    /// Zero extent.
    pub const ZERO: Size = Size { w: 0.0, h: 0.0 };

    /// Construct, clamping negative components to zero.
    #[must_use]
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            w: w.max(0.0),
            h: h.max(0.0),
        }
    }

    /// Area in logical square units.
    #[must_use]
    pub fn area(self) -> f32 {
        self.w * self.h
    }
}

/// An axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width (`>= 0`).
    pub w: f32,
    /// Height (`>= 0`).
    pub h: f32,
}

impl Rect {
    /// Empty rectangle at the origin.
    pub const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    /// Construct from origin plus size; negative extents clamp to zero.
    #[must_use]
    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Self {
            x: origin.x,
            y: origin.y,
            w: size.w,
            h: size.h,
        }
    }

    /// Construct from edges; inverted edges clamp to an empty rect.
    #[must_use]
    pub fn from_edges(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            x: left,
            y: top,
            w: (right - left).max(0.0),
            h: (bottom - top).max(0.0),
        }
    }

    /// Right edge.
    #[must_use]
    pub fn right(self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge.
    #[must_use]
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    /// Origin point.
    #[must_use]
    pub fn origin(self) -> Point {
        Point::new(self.x, self.y)
    }

    /// Size.
    #[must_use]
    pub fn size(self) -> Size {
        Size {
            w: self.w,
            h: self.h,
        }
    }

    /// Center point.
    #[must_use]
    pub fn center(self) -> Point {
        Point::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    /// Whether the rect has zero area.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// Whether `p` lies inside (left/top inclusive, right/bottom exclusive).
    #[must_use]
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// Intersection; empty when disjoint.
    #[must_use]
    pub fn intersect(self, other: Rect) -> Rect {
        Rect::from_edges(
            self.x.max(other.x),
            self.y.max(other.y),
            self.right().min(other.right()),
            self.bottom().min(other.bottom()),
        )
    }

    /// Bounding union.
    #[must_use]
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Rect::from_edges(
            self.x.min(other.x),
            self.y.min(other.y),
            self.right().max(other.right()),
            self.bottom().max(other.bottom()),
        )
    }

    /// Whether the rects overlap with nonzero area.
    #[must_use]
    pub fn overlaps(self, other: Rect) -> bool {
        !self.intersect(other).is_empty()
    }

    /// Grow (positive) or shrink (negative) uniformly on all sides.
    /// Used for effect sampling footprints (blur/shadow) and damage bounds.
    #[must_use]
    pub fn inflate(self, px: f32) -> Rect {
        Rect {
            x: self.x - px,
            y: self.y - px,
            w: (self.w + 2.0 * px).max(0.0),
            h: (self.h + 2.0 * px).max(0.0),
        }
    }

    /// Translate by an offset.
    #[must_use]
    pub fn translate(self, dx: f32, dy: f32) -> Rect {
        Rect {
            x: self.x + dx,
            y: self.y + dy,
            w: self.w,
            h: self.h,
        }
    }

    /// Inset (positive shrinks) or outset (negative grows) by `insets`.
    #[must_use]
    pub fn inset(self, insets: Insets) -> Rect {
        Rect::from_edges(
            self.x + insets.left,
            self.y + insets.top,
            self.right() - insets.right,
            self.bottom() - insets.bottom,
        )
    }

    /// Transform by an affine matrix, returning the bounding box.
    #[must_use]
    pub fn transform(self, t: Transform2D) -> Rect {
        let corners = [
            t.apply(self.origin()),
            t.apply(Point::new(self.right(), self.y)),
            t.apply(Point::new(self.x, self.bottom())),
            t.apply(Point::new(self.right(), self.bottom())),
        ];
        let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
        let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for p in corners {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
        Rect::from_edges(min_x, min_y, max_x, max_y)
    }
}

/// Edge offsets, e.g. padding, borders, or safe-area insets.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Insets {
    /// Left offset.
    pub left: f32,
    /// Top offset.
    pub top: f32,
    /// Right offset.
    pub right: f32,
    /// Bottom offset.
    pub bottom: f32,
}

impl Insets {
    /// Uniform offset on all sides.
    #[must_use]
    pub const fn uniform(v: f32) -> Self {
        Self {
            left: v,
            top: v,
            right: v,
            bottom: v,
        }
    }

    /// Horizontal/vertical pairs.
    #[must_use]
    pub const fn hv(h: f32, v: f32) -> Self {
        Self {
            left: h,
            top: v,
            right: h,
            bottom: v,
        }
    }

    /// Total horizontal extent.
    #[must_use]
    pub fn horizontal(self) -> f32 {
        self.left + self.right
    }

    /// Total vertical extent.
    #[must_use]
    pub fn vertical(self) -> f32 {
        self.top + self.bottom
    }
}

/// Per-corner radii for rounded rectangles and clips.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CornerRadii {
    /// Top-left radius.
    pub top_left: f32,
    /// Top-right radius.
    pub top_right: f32,
    /// Bottom-right radius.
    pub bottom_right: f32,
    /// Bottom-left radius.
    pub bottom_left: f32,
}

impl CornerRadii {
    /// Uniform radius on all corners.
    #[must_use]
    pub const fn uniform(r: f32) -> Self {
        Self {
            top_left: r,
            top_right: r,
            bottom_right: r,
            bottom_left: r,
        }
    }

    /// Clamp every radius to `>= 0` and `<= half the smaller rect dimension`.
    #[must_use]
    pub fn clamp_to(self, rect: Rect) -> Self {
        let max = rect.w.min(rect.h) * 0.5;
        let clamp = |r: f32| r.clamp(0.0, max.max(0.0));
        Self {
            top_left: clamp(self.top_left),
            top_right: clamp(self.top_right),
            bottom_right: clamp(self.bottom_right),
            bottom_left: clamp(self.bottom_left),
        }
    }
}

/// 2D affine transform in row-major order:
///
/// ```text
/// [ a c tx ]
/// [ b d ty ]
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform2D {
    /// Matrix components.
    pub a: f32,
    /// Matrix components.
    pub b: f32,
    /// Matrix components.
    pub c: f32,
    /// Matrix components.
    pub d: f32,
    /// Translation.
    pub tx: f32,
    /// Translation.
    pub ty: f32,
}

impl Transform2D {
    /// Identity transform.
    pub const IDENTITY: Transform2D = Transform2D {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// Pure translation.
    #[must_use]
    pub const fn translate(tx: f32, ty: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx,
            ty,
        }
    }

    /// Uniform or non-uniform scale about the origin.
    #[must_use]
    pub const fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Compose: apply `self` first, then `after`.
    #[must_use]
    pub fn then(self, after: Transform2D) -> Transform2D {
        Transform2D {
            a: after.a * self.a + after.c * self.b,
            b: after.b * self.a + after.d * self.b,
            c: after.a * self.c + after.c * self.d,
            d: after.b * self.c + after.d * self.d,
            tx: after.a * self.tx + after.c * self.ty + after.tx,
            ty: after.b * self.tx + after.d * self.ty + after.ty,
        }
    }

    /// Apply to a point.
    #[must_use]
    pub fn apply(self, p: Point) -> Point {
        Point {
            x: self.a * p.x + self.c * p.y + self.tx,
            y: self.b * p.x + self.d * p.y + self.ty,
        }
    }

    /// Whether the transform is axis-aligned (no rotation/skew), so clips and
    /// hit regions stay rectangles.
    #[must_use]
    pub fn is_axis_aligned(self) -> bool {
        self.b == 0.0 && self.c == 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_union_contains() {
        let a = Rect::from_edges(0.0, 0.0, 10.0, 10.0);
        let b = Rect::from_edges(5.0, 5.0, 15.0, 15.0);
        assert_eq!(a.intersect(b), Rect::from_edges(5.0, 5.0, 10.0, 10.0));
        assert_eq!(a.union(b), Rect::from_edges(0.0, 0.0, 15.0, 15.0));
        assert!(a.contains(Point::new(0.0, 0.0)));
        assert!(!a.contains(Point::new(10.0, 10.0)));
        assert!(a.overlaps(b));
        assert!(!a.overlaps(Rect::from_edges(20.0, 20.0, 30.0, 30.0)));
    }

    #[test]
    fn inverted_edges_yield_empty() {
        let r = Rect::from_edges(10.0, 10.0, 0.0, 0.0);
        assert!(r.is_empty());
        assert_eq!(
            r.union(Rect::from_edges(2.0, 2.0, 4.0, 4.0)),
            Rect::from_edges(2.0, 2.0, 4.0, 4.0)
        );
    }

    #[test]
    fn transform_bounding_box() {
        let r = Rect::from_edges(0.0, 0.0, 10.0, 5.0);
        let moved = r.transform(Transform2D::translate(3.0, 4.0));
        assert_eq!(moved, Rect::from_edges(3.0, 4.0, 13.0, 9.0));
        let scaled = r.transform(Transform2D::scale(2.0, 2.0));
        assert_eq!(scaled, Rect::from_edges(0.0, 0.0, 20.0, 10.0));
    }

    #[test]
    fn composition_order() {
        let t = Transform2D::translate(1.0, 0.0).then(Transform2D::scale(2.0, 2.0));
        assert_eq!(t.apply(Point::new(1.0, 1.0)), Point::new(4.0, 2.0));
    }

    #[test]
    fn corner_radii_clamp() {
        let r = Rect::from_edges(0.0, 0.0, 10.0, 8.0);
        let radii = CornerRadii::uniform(99.0).clamp_to(r);
        assert_eq!(radii, CornerRadii::uniform(4.0));
    }
}
