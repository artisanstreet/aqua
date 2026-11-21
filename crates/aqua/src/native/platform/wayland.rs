pub use aqua_shell_wayland::*;

pub use aqua_portal::open;
pub use aqua_shell::fallback::icon;

#[path = "wayland/dialogs.rs"]
pub mod dialogs;
#[path = "wayland/prefs.rs"]
pub mod prefs;

pub mod capability {
    pub use aqua_shell_wayland::capability::material_capability;

    pub fn query_preferences() -> aqua_protocol::prefs::SystemPrefs {
        super::prefs::query()
    }
}
