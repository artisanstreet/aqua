#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ButtonStrip {
    pub container_height: f64,
    pub container_y: f64,
}

pub fn centred_buttons(band: f64, button_height: f64, window_height: f64) -> ButtonStrip {
    let inset = ((band - button_height) / 2.0).max(0.0);
    let container_height = button_height + inset;
    ButtonStrip {
        container_height,
        container_y: window_height - container_height,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleClick {
    Zoom,
    Minimize,
    Nothing,
}

pub fn double_click(action: Option<&str>, legacy_minimize: Option<bool>) -> DoubleClick {
    match (action, legacy_minimize) {
        (Some("Minimize"), _) => DoubleClick::Minimize,
        (Some("None"), _) => DoubleClick::Nothing,
        (Some(_), _) => DoubleClick::Zoom,
        (None, Some(true)) => DoubleClick::Minimize,
        (None, _) => DoubleClick::Zoom,
    }
}
