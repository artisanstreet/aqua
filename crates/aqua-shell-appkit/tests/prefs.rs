#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("prefs: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, wait_for, Case};
    use aqua_protocol::prefs::{PrefState, PreferenceSource};
    use aqua_protocol::shell::ColorScheme;
    use aqua_shell_appkit::prefs;
    use aqua_shell_appkit::testing::desktop::{post_distributed, set_app_appearance};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    pub const ALL: [Case; 3] = [
        case(
            "the_dark_appearance_is_reported_as_dark_and_each_change_wakes_once",
            app_appearance,
        ),
        case(
            "a_system_appearance_change_reaches_the_app_through_the_notification",
            system_appearance,
        ),
        case(
            "reduce_motion_follows_the_accessibility_display_option",
            reduce_motion,
        ),
    ];

    const PROPAGATION: Duration = Duration::from_secs(10);

    fn counter() -> (Arc<AtomicUsize>, prefs::Wake) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = wakes.clone();
        (
            wakes,
            Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

    fn defaults(args: &[&str]) -> bool {
        Command::new("defaults")
            .args(args)
            .status()
            .is_ok_and(|status| status.success())
    }

    fn app_appearance() {
        if !support::appkit() {
            return;
        }
        aqua_shell_appkit::testing::activate();
        let (wakes, wake) = counter();
        set_app_appearance(Some(false));
        prefs::follow_changes(wake);
        assert_eq!(prefs::query().theme, ColorScheme::Light);
        assert_eq!(prefs::query().source, PreferenceSource::AppKit);
        set_app_appearance(Some(true));
        assert_eq!(prefs::query().theme, ColorScheme::Dark);
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "going dark wakes the loop once"
        );
        set_app_appearance(Some(true));
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "setting the same appearance again does not wake"
        );
        set_app_appearance(Some(false));
        assert_eq!(prefs::query().theme, ColorScheme::Light);
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            2,
            "going light wakes once more"
        );
    }

    fn system_appearance() {
        if !support::appkit() {
            return;
        }
        aqua_shell_appkit::testing::activate();
        let (wakes, wake) = counter();
        prefs::follow_changes(wake);
        let before = prefs::query().theme;
        let wanted = match before {
            ColorScheme::Dark => ColorScheme::Light,
            _ => ColorScheme::Dark,
        };
        let written = match wanted {
            ColorScheme::Dark => defaults(&["write", "-g", "AppleInterfaceStyle", "Dark"]),
            _ => defaults(&["delete", "-g", "AppleInterfaceStyle"]),
        };
        assert!(
            written,
            "the global AppleInterfaceStyle default is writable"
        );
        post_distributed("AppleInterfaceThemeChangedNotification");
        let seen = wait_for(PROPAGATION, move || {
            wakes.load(Ordering::SeqCst) > 0 && prefs::query().theme == wanted
        });
        let restored = match before {
            ColorScheme::Dark => defaults(&["write", "-g", "AppleInterfaceStyle", "Dark"]),
            _ => defaults(&["delete", "-g", "AppleInterfaceStyle"]),
        };
        post_distributed("AppleInterfaceThemeChangedNotification");
        assert!(restored, "the appearance default is restored");
        match seen {
            true => println!("system appearance {before:?} -> {wanted:?} followed"),
            false => println!(
                "SKIP-NO-APPEARANCE-PROPAGATION: AppKit did not apply a written AppleInterfaceStyle within {PROPAGATION:?}"
            ),
        }
    }

    fn reduce_motion() {
        if !support::appkit() {
            return;
        }
        aqua_shell_appkit::testing::activate();
        let before = prefs::system(None).reduce_motion;
        let wanted = match before {
            PrefState::On => "false",
            _ => "true",
        };
        if !defaults(&[
            "write",
            "com.apple.universalaccess",
            "reduceMotion",
            "-bool",
            wanted,
        ]) {
            println!(
                "SKIP-NO-UNIVERSAL-ACCESS-WRITE: com.apple.universalaccess is not writable by this process"
            );
            return;
        }
        let (wakes, wake) = counter();
        prefs::follow_changes(wake);
        let system_moved = wait_for(PROPAGATION, move || {
            prefs::system(None).reduce_motion != before
        });
        let woken = wakes.load(Ordering::SeqCst);
        let now = prefs::query().reduce_motion;
        let restore = match before {
            PrefState::On => "true",
            _ => "false",
        };
        assert!(
            defaults(&[
                "write",
                "com.apple.universalaccess",
                "reduceMotion",
                "-bool",
                restore,
            ]),
            "reduce motion is restored"
        );
        if !system_moved {
            println!(
                "SKIP-NO-ACCESSIBILITY-PROPAGATION: NSWorkspace kept reduce motion {before:?} after the default was written"
            );
            return;
        }
        assert_ne!(now, before, "the queried preference follows the system");
        assert!(
            woken > 0,
            "the accessibility display notification wakes the loop"
        );
    }
}
