use std::cell::RefCell;

#[derive(Debug)]
pub struct Memo<K, V> {
    entries: Vec<Option<(K, V)>>,
    samples: u32,
    misses: u32,
}

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> Memo<K, V> {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            samples: 0,
            misses: 0,
        }
    }
}

impl<K: Copy + PartialEq, V: Copy> Memo<K, V> {
    fn index(hash: u64, slots: usize) -> usize {
        (hash >> (64 - slots.ilog2())) as usize
    }

    pub fn slots(&self) -> usize {
        self.entries.len()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[inline]
    pub fn lookup(&mut self, key: K, hash: impl Fn(K) -> u64, compute: impl FnOnce() -> V) -> V {
        if self.entries.is_empty() {
            self.entries = vec![None; 256];
        }
        let index = Self::index(hash(key), self.entries.len());
        let value = match self.entries[index] {
            Some((existing, value)) if existing == key => value,
            _ => {
                self.misses += 1;
                let value = compute();
                self.entries[index] = Some((key, value));
                value
            }
        };
        self.samples += 1;
        if self.samples == 4096 {
            if self.misses > 128 && self.entries.len() < 65536 {
                let slots = self.entries.len() * 4;
                let entries = std::mem::replace(&mut self.entries, vec![None; slots]);
                for (key, value) in entries.into_iter().flatten() {
                    self.entries[Self::index(hash(key), slots)] = Some((key, value));
                }
            }
            self.samples = 0;
            self.misses = 0;
        }
        value
    }
}

thread_local! {
    static SRGB: RefCell<Memo<u32, [f32; 3]>> = const { RefCell::new(Memo::new()) };
}

pub(super) fn decode_srgb([r, g, b]: [u8; 3]) -> [f32; 3] {
    let (l, a, b) = super::linear_srgb_to_oklab(super::srgb8_to_linear([r, g, b]));
    let (c, h) = super::oklab_to_lch(a, b);
    [l, c.max(0.0), h.rem_euclid(360.0)]
}

#[inline]
fn lookup_srgb(memo: &mut Memo<u32, [f32; 3]>, color: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = color;
    let key = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
    memo.lookup(
        key,
        |key| u64::from(key.wrapping_mul(0x9e3779b9)) << 32,
        || decode_srgb(color),
    )
}

pub(super) fn srgb(color: [u8; 3]) -> [f32; 3] {
    SRGB.with_borrow_mut(|memo| lookup_srgb(memo, color))
}

pub(super) fn extend_srgb(
    output: &mut Vec<super::Oklch>,
    colors: impl IntoIterator<Item = ([u8; 3], f32)>,
) {
    let mut memo = SRGB.with_borrow_mut(std::mem::take);
    output.extend(colors.into_iter().map(|(color, alpha)| {
        let [l, c, h] = lookup_srgb(&mut memo, color);
        super::Oklch {
            l,
            c,
            h,
            alpha: alpha.clamp(0.0, 1.0),
        }
    }));
    SRGB.with_borrow_mut(|cached| *cached = memo);
}

#[cfg(test)]
#[path = "../../tests/color/memo.rs"]
mod tests;
