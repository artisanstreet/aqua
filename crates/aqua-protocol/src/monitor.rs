use crate::geometry::{LogicalSize, PhysicalPosition};
use aqua_core::Rect;

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub name: Option<String>,
    pub physical_bounds: Rect,
    pub physical_work_area: Rect,
    pub scale_factor: f64,
}

impl DisplayInfo {
    pub fn fit_window(&self, desired: LogicalSize, minimum: LogicalSize) -> Option<LogicalSize> {
        if !self.scale_factor.is_finite()
            || self.scale_factor <= 0.0
            || desired.is_empty()
            || minimum.is_empty()
            || ![desired.width, desired.height, minimum.width, minimum.height]
                .into_iter()
                .all(f64::is_finite)
        {
            return None;
        }
        Some(LogicalSize::new(
            desired
                .width
                .min(f64::from(self.physical_work_area.w) / self.scale_factor)
                .max(minimum.width),
            desired
                .height
                .min(f64::from(self.physical_work_area.h) / self.scale_factor)
                .max(minimum.height),
        ))
    }

    pub fn center_physical(&self, width: u32, height: u32) -> PhysicalPosition {
        PhysicalPosition {
            x: (self.physical_work_area.x + (self.physical_work_area.w - width as f32) * 0.5)
                .round() as i32,
            y: (self.physical_work_area.y + (self.physical_work_area.h - height as f32) * 0.5)
                .round() as i32,
        }
    }
}
