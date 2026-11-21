pub use aqua_shell_appkit::*;

pub use aqua_shell::fallback::{icon, open};

#[path = "appkit/dialogs.rs"]
pub mod dialogs;

pub mod prefs {
    pub fn query() -> aqua_protocol::prefs::SystemPrefs {
        aqua_shell_appkit::capability::query_preferences()
    }
}
