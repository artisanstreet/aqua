use aqua_core::{EditingConventions, Modifiers};

#[must_use]
pub fn conventions() -> EditingConventions {
    if cfg!(target_os = "macos") {
        EditingConventions::Mac
    } else {
        EditingConventions::Pc
    }
}

#[must_use]
pub fn primary() -> Modifiers {
    conventions().shortcut()
}
