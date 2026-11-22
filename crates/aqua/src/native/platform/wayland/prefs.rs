use std::sync::{Arc, OnceLock};

use aqua_core::ClickSettings;
use aqua_portal::prefs::Preferences;
use aqua_protocol::prefs::SystemPrefs;
use aqua_shell_wayland::Waker;

static PREFERENCES: OnceLock<Preferences> = OnceLock::new();
static WAKER: OnceLock<Waker> = OnceLock::new();

fn preferences() -> &'static Preferences {
    PREFERENCES.get_or_init(|| {
        Preferences::start(Arc::new(|| {
            if let Some(waker) = WAKER.get() {
                waker.wake();
            }
        }))
    })
}

pub fn follow_changes(waker: Waker) {
    let _ = WAKER.set(waker);
    preferences();
}

pub fn query() -> SystemPrefs {
    preferences().query()
}

pub fn click_settings() -> ClickSettings {
    preferences().click_settings()
}
