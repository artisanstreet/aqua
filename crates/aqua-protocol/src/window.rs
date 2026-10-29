use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub NonZeroU64);

impl WindowId {
    pub fn new(raw: u64) -> Option<Self> {
        NonZeroU64::new(raw).map(WindowId)
    }

    pub fn raw(self) -> u64 {
        self.0.get()
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameReason: u32 {
        const INPUT      = 0b0000_0001;
        const DAMAGE     = 0b0000_0010;
        const ANIMATION  = 0b0000_0100;
        const MEDIA      = 0b0000_1000;
        const EXPOSE     = 0b0000_0001_0000;
        const RESIZE     = 0b0000_0010_0000;
        const MATERIAL   = 0b0000_0100_0000;
        const EXPLICIT   = 0b0000_1000_0000;
    }
}
