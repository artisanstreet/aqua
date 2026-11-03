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

    pub fn from_monitor(monitor: &winit::monitor::MonitorHandle) -> Self {
        let position = monitor.position();
        let size = monitor.size();
        let bounds = Rect {
            x: position.x as f32,
            y: position.y as f32,
            w: size.width as f32,
            h: size.height as f32,
        };
        Self {
            name: monitor.name(),
            physical_bounds: bounds,
            physical_work_area: work_area(monitor).unwrap_or(bounds),
            scale_factor: monitor.scale_factor(),
        }
    }
}

pub fn current(window: &winit::window::Window) -> Option<DisplayInfo> {
    window
        .current_monitor()
        .or_else(|| window.primary_monitor())
        .as_ref()
        .map(DisplayInfo::from_monitor)
}

#[cfg(windows)]
fn work_area(monitor: &winit::monitor::MonitorHandle) -> Option<Rect> {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
    use winit::platform::windows::MonitorHandleExtWindows;
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(HMONITOR(monitor.hmonitor() as *mut _), &mut info) }.as_bool() {
        let bounds = info.rcWork;
        Some(Rect::from_edges(
            bounds.left as f32,
            bounds.top as f32,
            bounds.right as f32,
            bounds.bottom as f32,
        ))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn work_area(_: &winit::monitor::MonitorHandle) -> Option<Rect> {
    None
}
