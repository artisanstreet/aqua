use aqua_core::{Key, NamedKey};
use xkbcommon::xkb::Keysym;

pub(crate) fn named_key(keysym: Keysym) -> Option<NamedKey> {
    let raw = keysym.raw();
    match keysym {
        Keysym::Escape => Some(NamedKey::Escape),
        Keysym::Return | Keysym::KP_Enter => Some(NamedKey::Enter),
        Keysym::Tab | Keysym::ISO_Left_Tab | Keysym::KP_Tab => Some(NamedKey::Tab),
        Keysym::BackSpace => Some(NamedKey::Backspace),
        Keysym::Delete | Keysym::KP_Delete => Some(NamedKey::Delete),
        Keysym::Insert | Keysym::KP_Insert => Some(NamedKey::Insert),
        Keysym::Menu => Some(NamedKey::ContextMenu),
        Keysym::Left | Keysym::KP_Left => Some(NamedKey::Left),
        Keysym::Right | Keysym::KP_Right => Some(NamedKey::Right),
        Keysym::Up | Keysym::KP_Up => Some(NamedKey::Up),
        Keysym::Down | Keysym::KP_Down => Some(NamedKey::Down),
        Keysym::Home | Keysym::KP_Home => Some(NamedKey::Home),
        Keysym::End | Keysym::KP_End => Some(NamedKey::End),
        Keysym::Prior | Keysym::KP_Prior => Some(NamedKey::PageUp),
        Keysym::Next | Keysym::KP_Next => Some(NamedKey::PageDown),
        _ if (Keysym::F1.raw()..=Keysym::F24.raw()).contains(&raw) => {
            Some(NamedKey::Function((raw - Keysym::F1.raw() + 1) as u8))
        }
        _ => None,
    }
}

pub(crate) fn is_modifier(keysym: Keysym) -> bool {
    matches!(
        keysym,
        Keysym::Shift_L
            | Keysym::Shift_R
            | Keysym::Control_L
            | Keysym::Control_R
            | Keysym::Alt_L
            | Keysym::Alt_R
            | Keysym::Meta_L
            | Keysym::Meta_R
            | Keysym::Super_L
            | Keysym::Super_R
            | Keysym::Hyper_L
            | Keysym::Hyper_R
            | Keysym::ISO_Level3_Shift
            | Keysym::ISO_Level5_Shift
            | Keysym::Mode_switch
    )
}

pub(crate) fn resolve(keysym: Keysym) -> Key {
    match (named_key(keysym), keysym.key_char()) {
        (Some(named), _) => Key::Named(named),
        (None, Some(c)) if !c.is_control() => Key::Char(c),
        (None, _) => Key::Named(NamedKey::Unidentified),
    }
}
