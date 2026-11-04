pub const ROOT_FONT_PX: f32 = 16.0;
pub const MEDIUM_FONT_PX: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Px(f32),
    Em(f32),
    Percent(f32),
}

pub fn parse_length(value: &str) -> Option<Length> {
    let value = value.trim();
    let end = value
        .char_indices()
        .find(|&(index, c)| {
            !(c.is_ascii_digit()
                || c == '.'
                || (index == 0 && (c == '-' || c == '+'))
                || (matches!(c, 'e' | 'E')
                    && index > 0
                    && value[index + 1..]
                        .chars()
                        .next()
                        .is_some_and(|n| n.is_ascii_digit() || n == '-' || n == '+')))
        })
        .map_or(value.len(), |(index, _)| index);
    let number: f32 = value[..end].parse().ok()?;
    match value[end..].trim() {
        "" | "px" => Some(Length::Px(number)),
        "pt" => Some(Length::Px(number * 4.0 / 3.0)),
        "pc" => Some(Length::Px(number * 16.0)),
        "in" => Some(Length::Px(number * 96.0)),
        "cm" => Some(Length::Px(number * 96.0 / 2.54)),
        "mm" => Some(Length::Px(number * 96.0 / 25.4)),
        "q" | "Q" => Some(Length::Px(number * 96.0 / 101.6)),
        "em" => Some(Length::Em(number)),
        "ex" => Some(Length::Em(number * 0.5)),
        "rem" => Some(Length::Px(number * ROOT_FONT_PX)),
        "%" => Some(Length::Percent(number / 100.0)),
        _ => None,
    }
}

pub fn resolve(length: Length, font_px: f32) -> Option<f32> {
    match length {
        Length::Px(px) => Some(px),
        Length::Em(em) => Some(em * font_px),
        Length::Percent(_) => None,
    }
}

pub fn font_size(value: &str, parent_px: f32) -> Option<f32> {
    let named = match value.trim() {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(MEDIUM_FONT_PX),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "xxx-large" => Some(48.0),
        "smaller" => Some(parent_px / 1.2),
        "larger" => Some(parent_px * 1.2),
        _ => None,
    };
    named
        .or_else(|| match parse_length(value)? {
            Length::Percent(fraction) => Some(parent_px * fraction),
            other => resolve(other, parent_px),
        })
        .filter(|size| *size > 0.0)
}

pub fn lengths(value: Option<&str>, font_px: f32) -> Vec<Option<f32>> {
    value
        .map(|value| {
            value
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|part| !part.is_empty())
                .map(|part| parse_length(part).and_then(|length| resolve(length, font_px)))
                .collect()
        })
        .unwrap_or_default()
}
