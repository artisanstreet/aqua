use aqua_core::Oklch;

const SLOTS: usize = 256;

#[derive(Clone, Copy)]
struct Entry {
    key: [u32; 4],
    value: [f32; 4],
    live: bool,
}

const EMPTY: Entry = Entry {
    key: [0; 4],
    value: [0.0; 4],
    live: false,
};

#[derive(Default)]
pub struct ColorMemo {
    premultiplied: Vec<Entry>,
    straight: Vec<Entry>,
}

fn key_of(color: Oklch) -> [u32; 4] {
    [
        color.l.to_bits(),
        color.c.to_bits(),
        color.h.to_bits(),
        color.alpha.to_bits(),
    ]
}

fn slot_of(key: [u32; 4]) -> usize {
    let mixed = key.iter().fold(0x9E37_79B9u32, |hash, word| {
        (hash ^ word).wrapping_mul(0x85EB_CA6B).rotate_left(13)
    });
    (mixed as usize) % SLOTS
}

fn lookup(
    table: &mut Vec<Entry>,
    color: Oklch,
    convert: impl FnOnce(Oklch) -> [f32; 4],
) -> [f32; 4] {
    if table.is_empty() {
        table.resize(SLOTS, EMPTY);
    }
    let key = key_of(color);
    let entry = &mut table[slot_of(key)];
    match entry.live && entry.key == key {
        true => entry.value,
        false => {
            let value = convert(color);
            *entry = Entry {
                key,
                value,
                live: true,
            };
            value
        }
    }
}

impl ColorMemo {
    pub fn premultiplied(&mut self, color: Oklch) -> [f32; 4] {
        lookup(&mut self.premultiplied, color, Oklch::to_gpu)
    }

    pub fn straight(&mut self, color: Oklch) -> [f32; 4] {
        lookup(&mut self.straight, color, Oklch::to_srgba)
    }
}
