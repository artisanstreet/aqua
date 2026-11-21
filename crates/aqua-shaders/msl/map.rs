#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Buffer(u32),
    Texture(u32),
    Sampler(u32),
}

pub const SET: u32 = 0;
pub const LANGUAGE: (u32, u32) = (2, 4);
pub const BINDINGS: u32 = 64;
pub const INSTANCES: u32 = 30;
pub const SECOND_INSTANCES: u32 = 29;

const STORAGE_TEXTURES: u32 = 8;

pub const fn slot(binding: u32) -> Option<Slot> {
    match binding {
        0..=15 => Some(Slot::Buffer(binding)),
        16..=23 => Some(Slot::Texture(binding - 16)),
        32..=47 => Some(Slot::Sampler(binding - 32)),
        48..=55 => Some(Slot::Texture(STORAGE_TEXTURES + binding - 48)),
        _ => None,
    }
}
