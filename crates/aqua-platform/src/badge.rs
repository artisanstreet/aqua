use aqua_core::Oklch;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TaskbarBadge {
    pub count: u32,
    pub color: Oklch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BadgeError {
    Unsupported,
    Native(String),
}

impl std::fmt::Display for BadgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "taskbar badge error: {self:?}")
    }
}

impl std::error::Error for BadgeError {}

impl TaskbarBadge {
    pub fn label(self) -> String {
        match self.count {
            0 => String::new(),
            1..=9 => self.count.to_string(),
            _ => "9+".into(),
        }
    }

    pub fn compose(self, size: u32, text_coverage: &[u8]) -> Option<Vec<u8>> {
        if !(8..=256).contains(&size) || text_coverage.len() != (size * size) as usize {
            return None;
        }
        let [red, green, blue, alpha] = self.color.to_srgb8();
        let [r, g, b] = aqua_core::color::srgb8_to_linear([red, green, blue]);
        let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let ink = if (luminance + 0.05) / 0.05 > 1.05 / (luminance + 0.05) {
            0.0
        } else {
            255.0
        };
        let radius = size as f32 * 0.5;
        let mut pixels = Vec::with_capacity(text_coverage.len() * 4);
        for (index, coverage) in text_coverage.iter().enumerate() {
            let x = (index as u32 % size) as f32 + 0.5 - radius;
            let y = (index as u32 / size) as f32 + 0.5 - radius;
            let disc = (radius - x.hypot(y) + 0.5).clamp(0.0, 1.0);
            let text = f32::from(*coverage) / 255.0;
            for color in [blue, green, red] {
                pixels.push((f32::from(color) * (1.0 - text) + ink * text).round() as u8);
            }
            pixels.push((disc * f32::from(alpha)).round() as u8);
        }
        Some(pixels)
    }
}

#[cfg(windows)]
mod native;

#[cfg(windows)]
pub use native::{install_taskbar_wake, set_badge};

#[cfg(not(windows))]
pub fn set_badge(_: &winit::window::Window, _: Option<TaskbarBadge>) -> Result<(), BadgeError> {
    Err(BadgeError::Unsupported)
}
