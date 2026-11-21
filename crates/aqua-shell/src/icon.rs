#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIcon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn straight_rgba(premultiplied_bgra: &[u8]) -> Vec<u8> {
    let (pixels, _) = premultiplied_bgra.as_chunks::<4>();
    let without_alpha = pixels.iter().all(|pixel| pixel[3] == 0);
    pixels
        .iter()
        .flat_map(|pixel| {
            let alpha = if without_alpha { 255 } else { pixel[3] };
            let straight = |channel: u8| match alpha {
                0 => 0,
                alpha => (u32::from(channel) * 255 / u32::from(alpha)).min(255) as u8,
            };
            [
                straight(pixel[2]),
                straight(pixel[1]),
                straight(pixel[0]),
                alpha,
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::straight_rgba;

    #[test]
    fn icon_pixels_become_straight_rgba_and_alpha_less_icons_turn_opaque() {
        assert_eq!(
            straight_rgba(&[0, 64, 128, 128, 0, 0, 0, 0]),
            [255, 127, 0, 128, 0, 0, 0, 0]
        );
        assert_eq!(
            straight_rgba(&[10, 20, 30, 0, 1, 2, 3, 0]),
            [30, 20, 10, 255, 3, 2, 1, 255]
        );
    }
}
