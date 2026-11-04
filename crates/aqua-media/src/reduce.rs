const MAX_LEVELS: u32 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reduced {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[must_use]
pub fn reduction_level(width: u32, height: u32, needed_width: f32, needed_height: f32) -> u32 {
    let needed_width = needed_width.max(1.0).ceil() as u32;
    let needed_height = needed_height.max(1.0).ceil() as u32;
    (1..=MAX_LEVELS)
        .take_while(|level| {
            (width >> level) >= needed_width.max(1) && (height >> level) >= needed_height.max(1)
        })
        .last()
        .unwrap_or(0)
}

#[must_use]
pub fn reduce(premultiplied: &[u8], width: u32, height: u32, level: u32) -> Reduced {
    (0..level).fold(
        Reduced {
            width,
            height,
            pixels: premultiplied.to_vec(),
        },
        |previous, _| halve(&previous),
    )
}

fn halve(source: &Reduced) -> Reduced {
    let width = (source.width / 2).max(1);
    let height = (source.height / 2).max(1);
    let texel = |x: u32, y: u32, channel: usize| {
        let x = x.min(source.width - 1) as usize;
        let y = y.min(source.height - 1) as usize;
        u32::from(source.pixels[(y * source.width as usize + x) * 4 + channel])
    };
    let pixels = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            (0..4).map(move |channel| {
                let sum = texel(x * 2, y * 2, channel)
                    + texel(x * 2 + 1, y * 2, channel)
                    + texel(x * 2, y * 2 + 1, channel)
                    + texel(x * 2 + 1, y * 2 + 1, channel);
                ((sum + 2) / 4) as u8
            })
        })
        .collect();
    Reduced {
        width,
        height,
        pixels,
    }
}
