//! Window identity + adapter-local sizes, over canonical core geometry.
//!
//! Canonical layout/scene geometry is `aqua_core::geometry` (`Point`, `Size`,
//! `Rect`, … — re-exported here for convenience). The types below are the
//! adapter-owned remainder per the contract §1 table:
//! - [`AquaWindowId`]: framework window identity (no core window id exists).
//! - [`LogicalSize`]/[`LogicalPosition`]: `f64` DIP sizes at the windowing
//!   boundary (core uses `f32`; conversion is explicit, never silent).
//! - [`PhysicalSize`]: pixel sizes at the rendering boundary (no core
//!   equivalent; surfaces are created from these).
//!
//! No `winit` dependency.

pub use aqua_core::geometry::{CornerRadii, Insets, Point, Rect, Size, Transform2D};

pub use aqua_protocol::WindowId as AquaWindowId;

/// Logical (DIP) 2D size at the windowing boundary (`f64`).
///
/// Convert to canonical [`Size`] (`f32`) before layout/scene work via
/// [`as_core_size`](Self::as_core_size).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalSize {
    pub width: f64,
    pub height: f64,
}

impl LogicalSize {
    pub fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    pub fn is_empty(self) -> bool {
        !(self.width > 0.0 && self.height > 0.0)
    }

    pub fn to_physical(self, scale: f64) -> PhysicalSize {
        PhysicalSize {
            width: (self.width * scale).round().max(0.0) as u32,
            height: (self.height * scale).round().max(0.0) as u32,
        }
    }

    /// Explicit narrowing conversion to the canonical layout type.
    pub fn as_core_size(self) -> Size {
        Size::new(self.width as f32, self.height as f32)
    }
}

impl From<Size> for LogicalSize {
    fn from(s: Size) -> Self {
        LogicalSize::new(f64::from(s.w), f64::from(s.h))
    }
}

/// Logical (DIP) position at the windowing boundary (`f64`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalPosition {
    pub x: f64,
    pub y: f64,
}

impl LogicalPosition {
    /// Explicit narrowing conversion to the canonical layout type.
    pub fn as_core_point(self) -> Point {
        Point::new(self.x as f32, self.y as f32)
    }
}

impl From<Point> for LogicalPosition {
    fn from(p: Point) -> Self {
        LogicalPosition {
            x: f64::from(p.x),
            y: f64::from(p.y),
        }
    }
}

/// Physical pixels at the rendering boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalSize {
    pub width: u32,
    pub height: u32,
}

impl PhysicalSize {
    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_to_physical_rounds() {
        let l = LogicalSize::new(100.0, 50.0);
        assert_eq!(
            l.to_physical(1.5),
            PhysicalSize {
                width: 150,
                height: 75
            }
        );
        assert!(LogicalSize::new(0.0, 10.0).is_empty());
        assert!(PhysicalSize {
            width: 0,
            height: 5
        }
        .is_empty());
    }

    #[test]
    fn core_conversions_are_explicit_and_narrowing() {
        let l = LogicalSize::new(800.0, 600.0);
        let core: Size = l.as_core_size();
        assert_eq!(core, Size::new(800.0, 600.0));
        assert_eq!(LogicalSize::from(core), l);
        let p = LogicalPosition { x: 1.5, y: 2.5 }.as_core_point();
        assert_eq!(p, Point::new(1.5, 2.5));
    }

    #[test]
    fn window_id_rejects_zero() {
        assert!(AquaWindowId::new(0).is_none());
        assert_eq!(AquaWindowId::new(7).unwrap().raw(), 7);
    }
}
