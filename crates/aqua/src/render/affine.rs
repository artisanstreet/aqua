use aqua_core::geometry::Rect;
use aqua_core::Transform2D;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub Transform2D);

impl Affine {
    #[must_use]
    pub fn device(scale: f32) -> Self {
        Self(Transform2D::scale(scale, scale))
    }

    #[must_use]
    pub fn nested(self, local: Transform2D) -> Self {
        Self(local.then(self.0))
    }

    #[must_use]
    pub fn point(self, x: f32, y: f32) -> (f32, f32) {
        let moved = self.0.apply(aqua_core::Point::new(x, y));
        (moved.x, moved.y)
    }

    #[must_use]
    pub fn rect(self, rect: Rect) -> Rect {
        let corners = [
            self.point(rect.x, rect.y),
            self.point(rect.right(), rect.y),
            self.point(rect.right(), rect.bottom()),
            self.point(rect.x, rect.bottom()),
        ];
        let (left, right) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), (x, _)| {
                (lo.min(*x), hi.max(*x))
            });
        let (top, bottom) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), (_, y)| {
                (lo.min(*y), hi.max(*y))
            });
        Rect::from_edges(left, top, right, bottom)
    }

    #[must_use]
    pub fn scale(self) -> f32 {
        let Transform2D { a, b, c, d, .. } = self.0;
        (a * d - b * c).abs().sqrt()
    }

    #[must_use]
    pub fn is_exact(self) -> bool {
        let Transform2D { a, b, c, d, .. } = self.0;
        (b == 0.0 && c == 0.0) || (a == 0.0 && d == 0.0)
    }

    #[must_use]
    pub fn is_uniform(self) -> bool {
        let Transform2D { a, b, c, d, .. } = self.0;
        let straight = b == 0.0 && c == 0.0 && (a.abs() - d.abs()).abs() < 1e-5;
        let quarter = a == 0.0 && d == 0.0 && (b.abs() - c.abs()).abs() < 1e-5;
        straight || quarter
    }

    #[must_use]
    pub fn finite(self) -> bool {
        let Transform2D { a, b, c, d, tx, ty } = self.0;
        [a, b, c, d, tx, ty].iter().all(|value| value.is_finite())
    }
}
