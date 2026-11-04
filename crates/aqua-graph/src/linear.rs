use aqua_core::geometry::{Rect, Transform2D};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Linear2 {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

impl Default for Linear2 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Linear2 {
    pub const IDENTITY: Linear2 = Linear2 {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
    };

    pub fn rotation(radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
        }
    }

    pub fn skew(x_radians: f32, y_radians: f32) -> Self {
        Self {
            a: 1.0,
            b: y_radians.tan(),
            c: x_radians.tan(),
            d: 1.0,
        }
    }

    pub fn scale(x: f32, y: f32) -> Self {
        Self {
            a: x,
            b: 0.0,
            c: 0.0,
            d: y,
        }
    }

    pub fn of(transform: Transform2D) -> Self {
        Self {
            a: transform.a,
            b: transform.b,
            c: transform.c,
            d: transform.d,
        }
    }

    pub fn then(self, after: Linear2) -> Self {
        Self {
            a: after.a * self.a + after.c * self.b,
            b: after.b * self.a + after.d * self.b,
            c: after.a * self.c + after.c * self.d,
            d: after.b * self.c + after.d * self.d,
        }
    }

    pub fn is_identity(self) -> bool {
        self == Self::IDENTITY
    }

    pub fn is_finite(self) -> bool {
        [self.a, self.b, self.c, self.d]
            .iter()
            .all(|value| value.is_finite())
    }

    pub fn determinant(self) -> f32 {
        self.a * self.d - self.b * self.c
    }

    pub fn scale_factor(self) -> f32 {
        self.determinant().abs().sqrt()
    }

    pub fn is_invertible(self) -> bool {
        self.is_finite() && self.determinant().abs() > 1.0e-12
    }

    pub fn inverse(self) -> Option<Self> {
        let det = self.determinant();
        (self.is_finite() && det.abs() > 1.0e-12).then(|| Self {
            a: self.d / det,
            b: -self.b / det,
            c: -self.c / det,
            d: self.a / det,
        })
    }

    pub fn apply(self, point: [f32; 2]) -> [f32; 2] {
        [
            self.a * point[0] + self.c * point[1],
            self.b * point[0] + self.d * point[1],
        ]
    }

    pub fn pack(self) -> [f32; 4] {
        [self.a, self.b, self.c, self.d]
    }

    pub fn cover(self, bounds: Rect) -> Rect {
        match self.is_identity() {
            true => bounds,
            false => {
                let center = bounds.center();
                let half = [bounds.w * 0.5, bounds.h * 0.5];
                let corners = [
                    [-half[0], -half[1]],
                    [half[0], -half[1]],
                    [-half[0], half[1]],
                    [half[0], half[1]],
                ]
                .map(|corner| self.apply(corner));
                let reach = corners.iter().fold([0.0f32; 2], |reach, corner| {
                    [reach[0].max(corner[0].abs()), reach[1].max(corner[1].abs())]
                });
                Rect {
                    x: center.x - reach[0],
                    y: center.y - reach[1],
                    w: reach[0] * 2.0,
                    h: reach[1] * 2.0,
                }
            }
        }
    }

    pub fn about(self, pivot: [f32; 2], bounds: Rect) -> Rect {
        let center = bounds.center();
        let moved = self.apply([center.x - pivot[0], center.y - pivot[1]]);
        Rect {
            x: pivot[0] + moved[0] - bounds.w * 0.5,
            y: pivot[1] + moved[1] - bounds.h * 0.5,
            ..bounds
        }
    }
}
