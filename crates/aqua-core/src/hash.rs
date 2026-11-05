use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

const MULTIPLIER: u64 = 0xf135_7aea_2e62_a9c5;

#[derive(Clone, Copy, Debug, Default)]
pub struct FastHasher {
    state: u64,
}

impl FastHasher {
    #[inline]
    fn mix(&mut self, word: u64) {
        self.state = (self.state.rotate_left(5) ^ word).wrapping_mul(MULTIPLIER);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.mix(u64::from_le_bytes(*chunk));
        }
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.mix(u64::from_le_bytes(word) ^ ((rest.len() as u64) << 56));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.mix(u64::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.mix(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.mix(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.mix(value);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.mix(value as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.state ^ (self.state >> 29)
    }
}

pub type FastBuild = BuildHasherDefault<FastHasher>;
pub type FastMap<K, V> = HashMap<K, V, FastBuild>;

#[cfg(test)]
mod tests {
    use std::hash::{BuildHasher, Hash};

    use super::*;

    fn hash_of<T: Hash + ?Sized>(value: &T) -> u64 {
        FastBuild::default().hash_one(value)
    }

    #[test]
    fn nearby_keys_spread_over_low_and_high_bits() {
        let hashes: Vec<u64> = (0..4096u32).map(|n| hash_of(&(n, 7u8, 3u64))).collect();
        let low: std::collections::HashSet<u64> = hashes.iter().map(|h| h & 0xfff).collect();
        let high: std::collections::HashSet<u64> = hashes.iter().map(|h| h >> 57).collect();
        assert!(low.len() > 2400, "{}", low.len());
        assert_eq!(high.len(), 128);
    }

    #[test]
    fn byte_slices_of_different_length_differ() {
        assert_ne!(hash_of(&[0u8; 3][..]), hash_of(&[0u8; 4][..]));
        assert_ne!(hash_of(&"ab"), hash_of(&"abc"));
    }
}
