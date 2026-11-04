use aqua_core::{InputEvent, Point, PointerEvent, ScrollDelta};
use aqua_protocol::AquaWindowId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Coalescible {
    Scroll { at: Point, delta: ScrollDelta },
    Move(PointerEvent),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    pub window: AquaWindowId,
    pub input: Coalescible,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Merge {
    Merged(Held),
    Split(Held, Held),
}

impl Coalescible {
    pub fn classify(input: &InputEvent) -> Option<Self> {
        match input {
            InputEvent::Scroll { at, delta } => Some(Self::Scroll {
                at: *at,
                delta: *delta,
            }),
            InputEvent::PointerMove(event) => Some(Self::Move(*event)),
            _ => None,
        }
    }

    pub fn into_input(self) -> InputEvent {
        match self {
            Self::Scroll { at, delta } => InputEvent::Scroll { at, delta },
            Self::Move(event) => InputEvent::PointerMove(event),
        }
    }
}

pub fn merge(held: Held, next: Held) -> Merge {
    let input = match (held.input, next.input) {
        _ if held.window != next.window => None,
        (Coalescible::Scroll { delta: first, .. }, Coalescible::Scroll { at, delta: second }) => {
            Some(Coalescible::Scroll {
                at,
                delta: ScrollDelta {
                    x: first.x + second.x,
                    y: first.y + second.y,
                },
            })
        }
        (Coalescible::Move(first), Coalescible::Move(second))
            if first.contact == second.contact
                && first.kind == second.kind
                && first.buttons == second.buttons =>
        {
            Some(Coalescible::Move(second))
        }
        _ => None,
    };
    match input {
        Some(input) => Merge::Merged(Held {
            window: next.window,
            input,
        }),
        None => Merge::Split(held, next),
    }
}
