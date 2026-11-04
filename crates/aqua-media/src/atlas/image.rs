#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageKey {
    pub asset: u64,
    pub frame: u32,
    pub width: u32,
    pub height: u32,
}
