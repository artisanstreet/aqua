use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OverflowMode {
    #[default]
    Clip,
    StartEllipsis,
    EndEllipsis,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverflowCluster {
    pub bytes: Range<usize>,
    pub advance: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverflowResult {
    pub source: Range<usize>,
    pub ellipsis: Option<EllipsisPosition>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EllipsisPosition {
    Start,
    End,
}

pub fn single_line_overflow(
    text: &str,
    clusters: &[OverflowCluster],
    available_width: f32,
    ellipsis_width: f32,
    mode: OverflowMode,
) -> OverflowResult {
    single_line_overflow_with_spacing(text, clusters, available_width, ellipsis_width, 0.0, mode)
}

pub fn single_line_overflow_with_spacing(
    text: &str,
    clusters: &[OverflowCluster],
    available_width: f32,
    ellipsis_width: f32,
    letter_spacing: f32,
    mode: OverflowMode,
) -> OverflowResult {
    let full = || OverflowResult {
        source: 0..text.len(),
        ellipsis: None,
    };
    if mode == OverflowMode::Clip {
        return full();
    }
    let mut width = 0.0;
    let mut boundary = 0;
    let mut overflow = false;
    match mode {
        OverflowMode::EndEllipsis => {
            for (index, cluster) in clusters.iter().enumerate() {
                if width + ellipsis_width < available_width {
                    boundary = cluster.bytes.start;
                }
                width += cluster.advance + if index == 0 { 0.0 } else { letter_spacing };
                if width.floor() > available_width {
                    overflow = true;
                    break;
                }
            }
            if !overflow {
                return full();
            }
            let prefix = &text[..boundary];
            let trimmed =
                prefix.trim_end_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation());
            let end = clusters
                .iter()
                .take_while(|cluster| cluster.bytes.end <= trimmed.len())
                .last()
                .map_or(0, |cluster| cluster.bytes.end);
            OverflowResult {
                source: 0..end,
                ellipsis: Some(EllipsisPosition::End),
            }
        }
        OverflowMode::StartEllipsis => {
            boundary = clusters.first().map_or(0, |cluster| cluster.bytes.end);
            for (index, cluster) in clusters.iter().rev().enumerate() {
                if width + ellipsis_width < available_width {
                    boundary = cluster.bytes.end;
                }
                width += cluster.advance + if index == 0 { 0.0 } else { letter_spacing };
                if width.floor() > available_width {
                    overflow = true;
                    break;
                }
            }
            if !overflow {
                return full();
            }
            OverflowResult {
                source: boundary..text.len(),
                ellipsis: Some(EllipsisPosition::Start),
            }
        }
        OverflowMode::Clip => full(),
    }
}
