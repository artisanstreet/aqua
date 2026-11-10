use std::sync::Arc;
use std::time::Duration;

use crate::{DisplayItem, Rect};

#[derive(Clone, Debug)]
pub struct ShapeProgram {
    pub name: Arc<str>,
    pub parameters: Arc<[f32]>,
    pub paint: fn(Rect, Rect, &[f32]) -> Vec<DisplayItem>,
}

impl PartialEq for ShapeProgram {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.parameters == other.parameters
            && std::ptr::fn_addr_eq(self.paint, other.paint)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeTransition {
    pub duration: Duration,
    pub easing: [f32; 4],
    pub vertical_reveal: bool,
}
