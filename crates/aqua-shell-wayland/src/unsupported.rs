#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    Position,
    Unminimize,
    Minimize,
    ConstrainedSize,
    Topmost,
    Activation,
    Lowering,
    RawMouse,
}

impl Unsupported {
    pub fn reason(self) -> &'static str {
        match self {
            Unsupported::Position => "global window position on Wayland",
            Unsupported::Unminimize => "restoring a minimized window",
            Unsupported::Minimize => "minimizing a window on this compositor",
            Unsupported::ConstrainedSize => "size of a maximized, tiled or fullscreen window",
            Unsupported::Topmost => "always-on-top windows",
            Unsupported::Activation => "activating a window on this compositor",
            Unsupported::Lowering => "sending a window to the back",
            Unsupported::RawMouse => "raw mouse input",
        }
    }
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unsupported on Wayland: {}", self.reason())
    }
}

impl std::error::Error for Unsupported {}
