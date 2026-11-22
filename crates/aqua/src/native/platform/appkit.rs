pub use aqua_shell_appkit::*;

pub mod prefs {
    use std::sync::Arc;

    use aqua_core::ClickSettings;
    use aqua_protocol::prefs::SystemPrefs;
    use aqua_shell_appkit::Waker;

    pub fn follow_changes(waker: Waker) {
        aqua_shell_appkit::prefs::follow_changes(Arc::new(move || waker.wake()));
    }

    pub fn query() -> SystemPrefs {
        aqua_shell_appkit::prefs::query()
    }

    pub fn click_settings() -> ClickSettings {
        aqua_shell_appkit::prefs::click_settings()
    }
}
