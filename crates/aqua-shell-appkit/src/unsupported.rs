#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    Activation,
    FullscreenSize,
    Minimize,
    Maximize,
}

impl Unsupported {
    pub fn reason(self) -> &'static str {
        match self {
            Unsupported::Activation => {
                "activating a window while another app is frontmost (macOS cooperative activation)"
            }
            Unsupported::FullscreenSize => "size of a full-screen window",
            Unsupported::Minimize => "minimizing a window that is not minimizable",
            Unsupported::Maximize => "zooming a window that is not maximizable",
        }
    }
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unsupported on macOS: {}", self.reason())
    }
}

impl std::error::Error for Unsupported {}
