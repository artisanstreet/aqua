use std::fmt::Debug;

pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

pub fn of<T: Debug>(value: &T) -> u64 {
    fnv1a(format!("{value:?}").as_bytes())
}

pub fn of_bytes(bytes: &[u8]) -> u64 {
    fnv1a(bytes)
}
