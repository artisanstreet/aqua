use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};

pub const FLAG_INCONTACT: u32 = 0x0004;
pub const FLAG_CANCELED: u32 = 0x8000;
pub const FLAG_UP: u32 = 0x0004_0000;
pub const PEN_BARREL: u32 = 0x0001;
pub const PEN_ERASER: u32 = 0x0004;

pub const BUTTON_PRIMARY: u8 = 0x01;
pub const BUTTON_SECONDARY: u8 = 0x02;
pub const BUTTON_ERASER: u8 = 0x20;

const BUTTONS: [(u8, PointerButton); 3] = [
    (BUTTON_PRIMARY, PointerButton::Primary),
    (BUTTON_SECONDARY, PointerButton::Secondary),
    (BUTTON_ERASER, PointerButton::Extra(5)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Touch,
    Pen,
}

impl Device {
    pub fn from_pointer_type(raw: i32) -> Option<Device> {
        match raw {
            2 => Some(Device::Touch),
            3 => Some(Device::Pen),
            _ => None,
        }
    }

    fn kind(self) -> PointerKind {
        match self {
            Device::Touch => PointerKind::Touch,
            Device::Pen => PointerKind::Pen,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub id: u32,
    pub device: Device,
    pub flags: u32,
    pub pen_flags: u32,
    pub position: Point,
}

pub fn contact_id(id: u32) -> u64 {
    u64::from(id) + 1
}

pub fn button_mask(sample: &Sample) -> u8 {
    let in_contact = sample.flags & FLAG_INCONTACT != 0 && sample.flags & FLAG_UP == 0;
    let pen = sample.device == Device::Pen;
    let tip = if pen && sample.pen_flags & PEN_ERASER != 0 {
        BUTTON_ERASER
    } else {
        BUTTON_PRIMARY
    };
    let barrel = if pen && sample.pen_flags & PEN_BARREL != 0 {
        BUTTON_SECONDARY
    } else {
        0
    };
    match in_contact {
        true => tip | barrel,
        false => 0,
    }
}

pub fn translate(previous: u8, sample: &Sample) -> (Vec<InputEvent>, u8) {
    let contact = contact_id(sample.id);
    match sample.flags & FLAG_CANCELED != 0 {
        true => (vec![InputEvent::PointerCancel { contact }], 0),
        false => {
            let mask = button_mask(sample);
            let event = |buttons| PointerEvent {
                contact,
                kind: sample.device.kind(),
                position: sample.position,
                buttons,
            };
            let released = BUTTONS
                .iter()
                .filter(|(bit, _)| previous & bit != 0 && mask & bit == 0)
                .map(|(bit, button)| InputEvent::PointerUp(event(previous & !bit), *button));
            let pressed = BUTTONS
                .iter()
                .filter(|(bit, _)| previous & bit == 0 && mask & bit != 0)
                .map(|(bit, button)| InputEvent::PointerDown(event(previous | bit), *button));
            let transitions: Vec<InputEvent> = released.chain(pressed).collect();
            match transitions.is_empty() {
                true => (vec![InputEvent::PointerMove(event(mask))], mask),
                false => (transitions, mask),
            }
        }
    }
}
