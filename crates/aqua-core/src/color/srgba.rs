use super::Oklch;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Srgba8(pub [u8; 4]);

impl Srgba8 {
    pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self([red, green, blue, alpha])
    }

    pub fn to_oklch(self) -> Oklch {
        let [red, green, blue, alpha] = self.0;
        Oklch::from_srgb8(red, green, blue, f32::from(alpha) / 255.0)
    }

    pub fn to_srgba(self) -> [f32; 4] {
        self.0.map(|channel| f32::from(channel) / 255.0)
    }
}
