#![cfg(target_os = "linux")]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aqua_portal::prefs::{Preferences, STARTUP_TIMEOUT};
use aqua_protocol::prefs::{PrefState, PreferenceSource, TextScale};
use aqua_protocol::ColorScheme;
use support::{desktop, Desktop, Setting, PORTAL_PATH, WAIT};

const APPEARANCE: &str = "org.freedesktop.appearance";
const GNOME: &str = "org.gnome.desktop.interface";

fn quiet() -> aqua_portal::Wake {
    Arc::new(|| {})
}

#[test]
fn dark_color_scheme_maps_to_dark_theme_and_portal_source() {
    let Some(bus) = support::bus() else { return };
    let (_portal, _) = desktop(
        &bus,
        Desktop {
            settings: Some(vec![
                (APPEARANCE, "color-scheme", Setting::Unsigned(1)),
                (APPEARANCE, "contrast", Setting::Unsigned(1)),
                (GNOME, "text-scaling-factor", Setting::Float(1.25)),
            ]),
            ..Desktop::default()
        },
    );
    let prefs = Preferences::start_on(&bus.bus(), quiet()).system();
    assert_eq!(prefs.theme, ColorScheme::Dark);
    assert_eq!(prefs.high_contrast, PrefState::On);
    assert_eq!(prefs.text_scale, TextScale::from_percent(125));
    assert_eq!(prefs.reduce_transparency, PrefState::Unknown);
    assert_eq!(prefs.source, PreferenceSource::Portal);
}

#[test]
fn reduced_motion_falls_back_to_gnome_enable_animations() {
    let Some(bus) = support::bus() else { return };
    let (_portal, _) = desktop(
        &bus,
        Desktop {
            settings: Some(vec![
                (APPEARANCE, "color-scheme", Setting::Unsigned(2)),
                (GNOME, "enable-animations", Setting::Bool(false)),
            ]),
            ..Desktop::default()
        },
    );
    let prefs = Preferences::start_on(&bus.bus(), quiet()).system();
    assert_eq!(prefs.theme, ColorScheme::Light);
    assert_eq!(prefs.reduce_motion, PrefState::On);
    assert_eq!(prefs.high_contrast, PrefState::Unknown);
}

#[test]
fn a_setting_change_signal_wakes_once_per_effective_change_and_updates_the_preferences() {
    let Some(bus) = support::bus() else { return };
    let (portal, _) = desktop(
        &bus,
        Desktop {
            settings: Some(vec![(APPEARANCE, "color-scheme", Setting::Unsigned(2))]),
            ..Desktop::default()
        },
    );
    let wakes = Arc::new(AtomicUsize::new(0));
    let (woken, wake_seen) = channel();
    let counter = wakes.clone();
    let preferences = Preferences::start_on(
        &bus.bus(),
        Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = woken.send(());
        }),
    );
    assert_eq!(preferences.system().theme, ColorScheme::Light);
    let change = |namespace: &str, key: &str, value: Setting| {
        portal.emit(
            PORTAL_PATH,
            "org.freedesktop.portal.Settings",
            "SettingChanged",
            &(namespace, key, value.value()),
        )
    };
    change("org.example.unrelated", "anything", Setting::Unsigned(9));
    change(APPEARANCE, "color-scheme", Setting::Unsigned(2));
    change(APPEARANCE, "color-scheme", Setting::Unsigned(1));
    wake_seen
        .recv_timeout(WAIT)
        .expect("the change wakes the loop");
    assert_eq!(preferences.system().theme, ColorScheme::Dark);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    change(APPEARANCE, "reduced-motion", Setting::Unsigned(1));
    wake_seen
        .recv_timeout(WAIT)
        .expect("the second change wakes the loop");
    assert_eq!(preferences.system().reduce_motion, PrefState::On);
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
}

#[test]
fn missing_portal_returns_fallback_prefs_within_the_startup_timeout() {
    let Some(bus) = support::bus() else { return };
    let started = Instant::now();
    let preferences = Preferences::start_on(&bus.bus(), quiet());
    assert!(started.elapsed() < Duration::from_millis(350));
    assert_eq!(preferences.system().source, PreferenceSource::Fallback);
    assert_eq!(preferences.system().theme, ColorScheme::Unknown);
}

#[test]
fn a_hung_settings_portal_falls_back_within_the_startup_timeout() {
    let Some(bus) = support::bus() else { return };
    let (_portal, _) = desktop(
        &bus,
        Desktop {
            settings: Some(vec![(APPEARANCE, "color-scheme", Setting::Unsigned(1))]),
            hung_settings: true,
            ..Desktop::default()
        },
    );
    let started = Instant::now();
    let preferences = Preferences::start_on(&bus.bus(), quiet());
    let elapsed = started.elapsed();
    assert!(elapsed >= STARTUP_TIMEOUT, "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(350), "{elapsed:?}");
    assert_eq!(preferences.system().source, PreferenceSource::Fallback);
}

#[test]
fn the_double_click_interval_follows_the_gnome_and_kde_settings() {
    let Some(bus) = support::bus() else { return };
    let (_gnome, _) = desktop(
        &bus,
        Desktop {
            settings: Some(vec![(
                "org.gnome.desktop.peripherals.mouse",
                "double-click",
                Setting::Signed(250),
            )]),
            ..Desktop::default()
        },
    );
    let gnome = Preferences::start_on(&bus.bus(), quiet()).click_settings();
    assert_eq!(gnome.interval(), Duration::from_millis(250));

    let Some(kde_bus) = support::bus() else {
        return;
    };
    let (_kde, _) = desktop(
        &kde_bus,
        Desktop {
            settings: Some(vec![(
                "org.kde.kdeglobals.KDE",
                "DoubleClickInterval",
                Setting::Text("600"),
            )]),
            ..Desktop::default()
        },
    );
    let kde = Preferences::start_on(&kde_bus.bus(), quiet()).click_settings();
    assert_eq!(kde.interval(), Duration::from_millis(600));
}
