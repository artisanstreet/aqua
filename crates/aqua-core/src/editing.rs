use crate::event::{Key, KeyEvent, Modifiers, NamedKey};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EditingConventions {
    #[default]
    Pc,
    Mac,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Backward,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Motion {
    Character,
    Word,
    Line,
    Paragraph,
    LineBoundary,
    Document,
    Page,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeletionUnit {
    Character,
    Word,
    LineBoundary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditCommand {
    Move {
        motion: Motion,
        direction: Direction,
        extend: bool,
    },
    Delete {
        unit: DeletionUnit,
        direction: Direction,
    },
    SelectAll,
    Undo,
    Redo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClipboardCommand {
    Copy,
    Cut,
    Paste,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    Plain,
    Control,
    Option,
    Command,
}

impl EditingConventions {
    #[must_use]
    pub fn shortcut(self) -> Modifiers {
        match self {
            Self::Pc => Modifiers::CONTROL,
            Self::Mac => Modifiers::LOGO,
        }
    }

    fn reach(self, modifiers: Modifiers) -> Option<Reach> {
        let held =
            |flags: Modifiers| modifiers == flags || modifiers == flags.union(Modifiers::SHIFT);
        match self {
            _ if held(Modifiers::NONE) => Some(Reach::Plain),
            Self::Pc if held(Modifiers::CONTROL) => Some(Reach::Control),
            Self::Mac if held(Modifiers::ALT) => Some(Reach::Option),
            Self::Mac if held(Modifiers::LOGO) => Some(Reach::Command),
            Self::Pc | Self::Mac => None,
        }
    }

    #[must_use]
    pub fn edit_command(self, key: &KeyEvent) -> Option<EditCommand> {
        let reach = self.reach(key.modifiers)?;
        let extend = key.modifiers.contains(Modifiers::SHIFT);
        let moved = |motion: Motion, direction: Direction| {
            Some(EditCommand::Move {
                motion,
                direction,
                extend,
            })
        };
        let deleted = |unit: DeletionUnit, direction: Direction| {
            Some(EditCommand::Delete { unit, direction })
        };
        let horizontal = |named: NamedKey| match named {
            NamedKey::Left | NamedKey::Backspace => Direction::Backward,
            _ => Direction::Forward,
        };
        let vertical = |named: NamedKey| match named {
            NamedKey::Up | NamedKey::PageUp | NamedKey::Home => Direction::Backward,
            _ => Direction::Forward,
        };
        match (reach, key.key) {
            (reach, Key::Named(named @ (NamedKey::Left | NamedKey::Right))) => {
                let motion = match reach {
                    Reach::Plain => Motion::Character,
                    Reach::Control | Reach::Option => Motion::Word,
                    Reach::Command => Motion::LineBoundary,
                };
                moved(motion, horizontal(named))
            }
            (reach, Key::Named(named @ (NamedKey::Up | NamedKey::Down))) => {
                let motion = match reach {
                    Reach::Plain => Motion::Line,
                    Reach::Control | Reach::Option => Motion::Paragraph,
                    Reach::Command => Motion::Document,
                };
                moved(motion, vertical(named))
            }
            (Reach::Plain, Key::Named(named @ (NamedKey::Home | NamedKey::End))) => {
                moved(Motion::LineBoundary, vertical(named))
            }
            (Reach::Control, Key::Named(named @ (NamedKey::Home | NamedKey::End))) => {
                moved(Motion::Document, vertical(named))
            }
            (Reach::Plain, Key::Named(named @ (NamedKey::PageUp | NamedKey::PageDown))) => {
                moved(Motion::Page, vertical(named))
            }
            (reach, Key::Named(named @ (NamedKey::Backspace | NamedKey::Delete))) => {
                let unit = match reach {
                    Reach::Plain => DeletionUnit::Character,
                    Reach::Control | Reach::Option => DeletionUnit::Word,
                    Reach::Command => DeletionUnit::LineBoundary,
                };
                deleted(unit, horizontal(named))
            }
            (Reach::Control | Reach::Command, Key::Char('a' | 'A')) => Some(EditCommand::SelectAll),
            (Reach::Control | Reach::Command, Key::Char('z' | 'Z')) if extend => {
                Some(EditCommand::Redo)
            }
            (Reach::Control | Reach::Command, Key::Char('z' | 'Z')) => Some(EditCommand::Undo),
            (Reach::Control, Key::Char('y' | 'Y')) => Some(EditCommand::Redo),
            _ => None,
        }
    }

    #[must_use]
    pub fn clipboard_command(self, key: &KeyEvent) -> Option<ClipboardCommand> {
        let reach = self.reach(key.modifiers)?;
        let shifted = key.modifiers == Modifiers::SHIFT;
        match (reach, key.key) {
            (Reach::Control, Key::Char('c' | 'C') | Key::Named(NamedKey::Insert))
            | (Reach::Command, Key::Char('c' | 'C')) => Some(ClipboardCommand::Copy),
            (Reach::Control | Reach::Command, Key::Char('x' | 'X')) => Some(ClipboardCommand::Cut),
            (Reach::Control | Reach::Command, Key::Char('v' | 'V')) => {
                Some(ClipboardCommand::Paste)
            }
            (Reach::Plain, Key::Named(NamedKey::Delete)) if shifted && self == Self::Pc => {
                Some(ClipboardCommand::Cut)
            }
            (Reach::Plain, Key::Named(NamedKey::Insert)) if shifted && self == Self::Pc => {
                Some(ClipboardCommand::Paste)
            }
            _ => None,
        }
    }
}
