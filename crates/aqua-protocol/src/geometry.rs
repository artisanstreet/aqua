use aqua_core::geometry::{Point, Size};

pub use crate::window::WindowId as AquaWindowId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalPosition {
    pub x: i32,
    pub y: i32,
}

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

    pub fn as_core_size(self) -> Size {
        Size::new(self.width as f32, self.height as f32)
    }
}

impl From<Size> for LogicalSize {
    fn from(s: Size) -> Self {
        LogicalSize::new(f64::from(s.w), f64::from(s.h))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalPosition {
    pub x: f64,
    pub y: f64,
}

impl LogicalPosition {
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
