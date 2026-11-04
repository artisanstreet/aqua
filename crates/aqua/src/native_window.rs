use aqua_core::{ClickSettings, CursorIcon};
use aqua_protocol::chrome::DragRegions;
use aqua_protocol::monitor::DisplayInfo;
use aqua_protocol::{BadgeError, TaskbarBadge};

pub trait NativeWindow {
    fn scale(&self) -> f64;
    fn inner_size(&self) -> (u32, u32);
    fn set_title(&self, title: &str);
    fn set_logical_size(&self, width: f64, height: f64);
    fn set_visible(&self, visible: bool);
    fn set_minimized(&self, minimized: bool);
    fn set_maximized(&self, maximized: bool);
    fn is_maximized(&self) -> bool;
    fn activate(&self);
    fn set_outer_position(&self, x: i32, y: i32);
    fn center(&self);
    fn set_cursor(&self, cursor: CursorIcon);
    fn set_drag_regions(&self, regions: &DragRegions);
    fn click_settings(&self) -> ClickSettings;
    fn display(&self) -> Option<DisplayInfo>;
    fn set_badge(&self, badge: Option<TaskbarBadge>) -> Result<(), BadgeError>;
    fn destroy(&self);
    fn attach_composition(&self, _content: &dyn std::any::Any) -> bool {
        false
    }
}
