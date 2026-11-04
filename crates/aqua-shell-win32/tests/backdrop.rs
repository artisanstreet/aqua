use aqua_platform::material::WindowMaterial;
use aqua_shell_win32::backdrop::{Decision, decide};

#[test]
fn opaque_frame_and_old_builds_fall_back() {
    assert_eq!(
        decide(WindowMaterial::Mica, Some(22631), false),
        Decision::Apply(WindowMaterial::Mica)
    );
    assert!(matches!(
        decide(WindowMaterial::Acrylic, Some(22631), true),
        Decision::Fallback(_)
    ));
    assert!(matches!(
        decide(WindowMaterial::Mica, Some(19045), false),
        Decision::Fallback(_)
    ));
    assert_eq!(
        decide(WindowMaterial::Mica, None, false),
        Decision::Apply(WindowMaterial::Mica)
    );
    assert_eq!(
        decide(WindowMaterial::None, Some(1), true),
        Decision::Apply(WindowMaterial::None)
    );
}
