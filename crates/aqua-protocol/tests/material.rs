use aqua_protocol::material::{decide, needs_alpha_surface, DwmSystemBackdrop, MaterialDecision};
use aqua_protocol::prefs::{prefers_opaque_frame, should_reduce_motion, PrefState, SystemPrefs};
use aqua_protocol::WindowMaterial;

fn with(high_contrast: PrefState, reduce_transparency: PrefState) -> SystemPrefs {
    SystemPrefs {
        high_contrast,
        reduce_transparency,
        ..SystemPrefs::fallback()
    }
}

#[test]
fn opaque_frame_and_old_builds_fall_back() {
    let plain = SystemPrefs::fallback();
    assert_eq!(
        decide(WindowMaterial::Mica, Some(22631), &plain),
        MaterialDecision::Apply(WindowMaterial::Mica)
    );
    assert!(matches!(
        decide(
            WindowMaterial::Acrylic,
            Some(22631),
            &with(PrefState::On, PrefState::Off)
        ),
        MaterialDecision::Fallback(_)
    ));
    assert!(matches!(
        decide(
            WindowMaterial::Mica,
            Some(22631),
            &with(PrefState::Off, PrefState::On)
        ),
        MaterialDecision::Fallback(_)
    ));
    assert!(matches!(
        decide(WindowMaterial::Mica, Some(19045), &plain),
        MaterialDecision::Fallback(_)
    ));
    assert_eq!(
        decide(WindowMaterial::Mica, None, &plain),
        MaterialDecision::Apply(WindowMaterial::Mica)
    );
    assert_eq!(
        decide(
            WindowMaterial::Solid,
            Some(1),
            &with(PrefState::On, PrefState::On)
        ),
        MaterialDecision::Apply(WindowMaterial::Solid)
    );
}

#[test]
fn backdrop_values_follow_the_dwm_enum() {
    assert_eq!(DwmSystemBackdrop::of(WindowMaterial::Solid) as i32, 1);
    assert_eq!(DwmSystemBackdrop::of(WindowMaterial::Mica) as i32, 2);
    assert_eq!(DwmSystemBackdrop::of(WindowMaterial::Acrylic) as i32, 3);
    assert_eq!(DwmSystemBackdrop::of(WindowMaterial::Tabbed) as i32, 4);
    assert!(needs_alpha_surface(WindowMaterial::Tabbed));
    assert!(!needs_alpha_surface(WindowMaterial::Solid));
}

#[test]
fn overrides_beat_system_readings_and_unknown_is_never_on() {
    let env = |name: &str| match name {
        "AQUA_PREF_REDUCED_MOTION" => Some("YES".to_owned()),
        "AQUA_PREF_HIGH_CONTRAST" => Some("0".to_owned()),
        "AQUA_PREF_REDUCED_TRANSPARENCY" => Some("maybe".to_owned()),
        _ => None,
    };
    let prefs = SystemPrefs::fallback().with_overrides(env);
    assert!(should_reduce_motion(&prefs));
    assert_eq!(prefs.high_contrast, PrefState::Off);
    assert_eq!(prefs.reduce_transparency, PrefState::Unknown);
    assert!(!prefers_opaque_frame(&prefs));
    let untouched = SystemPrefs::fallback().with_overrides(|_| None);
    assert_eq!(untouched, SystemPrefs::fallback());
    assert!(!should_reduce_motion(&untouched));
}
