use aqua_core::{Key, Modifiers, NamedKey};
use aqua_platform::events::{classify_logical_key, translate_physical_key, AquaKeyCode};
use winit::keyboard::{Key as WinitKey, KeyCode, NamedKey as WinitNamedKey, PhysicalKey};

#[test]
fn context_menu_key_survives_physical_and_logical_translation() {
    let physical = PhysicalKey::Code(KeyCode::ContextMenu);
    assert_eq!(translate_physical_key(physical), AquaKeyCode::ContextMenu);
    let (event, text) = classify_logical_key(
        physical,
        &WinitKey::Named(WinitNamedKey::ContextMenu),
        None,
        Modifiers::default(),
        false,
    );
    assert_eq!(event.key, Key::Named(NamedKey::ContextMenu));
    assert_eq!(text, None);
}
