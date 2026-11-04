use std::ops::Range;

use icu_segmenter::LineSegmenter;
use icu_segmenter::options::LineBreakOptions;
use unicode_bidi::{Level, ParagraphBidiInfo};

use crate::shape::{Cluster, ShapedParagraph};
use crate::style::{TextAlign, WrapMode};

const FIT_EPSILON: f32 = 0.001;

pub(crate) fn break_opportunities(text: &str, offset: usize) -> Vec<usize> {
    let segmenter = LineSegmenter::new_dictionary(LineBreakOptions::default());
    segmenter
        .segment_str(text)
        .map(|position| position + offset)
        .collect()
}

fn tab_aware_width(cluster: &Cluster, x: f32) -> f32 {
    if cluster.tab && cluster.advance > 0.0 {
        let stops = (x / cluster.advance + FIT_EPSILON).floor() + 1.0;
        stops * cluster.advance - x
    } else {
        cluster.advance
    }
}

pub(crate) fn break_lines(
    clusters: &[Cluster],
    breaks: &[usize],
    wrap: WrapMode,
    limit: Option<f32>,
) -> Vec<Range<usize>> {
    let count = clusters.len();
    let Some(limit) = limit.filter(|_| wrap != WrapMode::None) else {
        return std::iter::once(0..count).collect();
    };
    let allowed_after = |index: usize| breaks.binary_search(&clusters[index].end).is_ok();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut x = 0.0f32;
    let mut last_break: Option<usize> = None;
    let mut overflowed = false;
    let mut index = 0;
    while index < count {
        let cluster = &clusters[index];
        let width = tab_aware_width(cluster, x);
        let fits = x + width <= limit + FIT_EPSILON || cluster.blank;
        if !fits && index > start {
            match (wrap, last_break) {
                (WrapMode::Word | WrapMode::WordOrGlyph, Some(boundary)) => {
                    lines.push(start..boundary);
                    start = boundary;
                    index = boundary;
                    x = 0.0;
                    last_break = None;
                    overflowed = false;
                    continue;
                }
                (WrapMode::Word, None) => overflowed = true,
                _ => {
                    lines.push(start..index);
                    start = index;
                    x = 0.0;
                    last_break = None;
                    overflowed = false;
                    continue;
                }
            }
        }
        x += width;
        if index + 1 < count && allowed_after(index) {
            if overflowed {
                lines.push(start..index + 1);
                start = index + 1;
                x = 0.0;
                last_break = None;
                overflowed = false;
            } else {
                last_break = Some(index + 1);
            }
        }
        index += 1;
    }
    if start < count || lines.is_empty() {
        lines.push(start..count);
    }
    lines
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PlacedGlyph {
    pub cluster: usize,
    pub glyph: usize,
    pub x: f32,
    pub advance: f32,
    pub level: u8,
}

pub(crate) struct PlacedLine {
    pub clusters: Range<usize>,
    pub glyphs: Vec<PlacedGlyph>,
    pub content_width: f32,
    pub rtl: bool,
    pub blank_count: usize,
    pub trailing_blanks: usize,
    pub last_in_paragraph: bool,
}

pub(crate) fn place_line(
    paragraph: &ShapedParagraph,
    range: Range<usize>,
    last_in_paragraph: bool,
) -> PlacedLine {
    let clusters = &paragraph.clusters[range.clone()];
    let mut widths = Vec::with_capacity(clusters.len());
    let mut x = 0.0f32;
    for cluster in clusters {
        let width = tab_aware_width(cluster, x);
        widths.push(width);
        x += width;
    }
    let trailing = clusters.iter().rev().take_while(|c| c.blank).count();
    let base_level = u8::from(paragraph.base_rtl);
    let mut levels: Vec<u8> = clusters.iter().map(|c| c.level).collect();
    let count = levels.len();
    for level in &mut levels[count - trailing..] {
        *level = base_level;
    }
    let order: Vec<usize> = if levels.iter().all(|level| *level == 0) {
        (0..count).collect()
    } else {
        let bidi_levels: Vec<Level> = levels
            .iter()
            .map(|level| Level::new(*level).unwrap_or_else(|_| Level::ltr()))
            .collect();
        ParagraphBidiInfo::reorder_visual(&bidi_levels)
    };
    let trailing_width: f32 = widths[count - trailing..].iter().sum();
    let total: f32 = widths.iter().sum();
    let content_width = if last_in_paragraph {
        total
    } else {
        total - trailing_width
    };
    let mut glyphs = Vec::new();
    let mut pen = 0.0f32;
    for &position in &order {
        let cluster = &clusters[position];
        let width = widths[position];
        let run = &paragraph.runs[cluster.run];
        let indices = cluster.glyphs.clone();
        if cluster.tab {
            glyphs.push(PlacedGlyph {
                cluster: range.start + position,
                glyph: indices.start,
                x: pen,
                advance: width,
                level: levels[position],
            });
            pen += width;
            continue;
        }
        for glyph_index in indices {
            let glyph = &run.glyphs[glyph_index];
            glyphs.push(PlacedGlyph {
                cluster: range.start + position,
                glyph: glyph_index,
                x: pen,
                advance: glyph.advance,
                level: levels[position],
            });
            pen += glyph.advance;
        }
    }
    let blank_count = clusters[..count - trailing]
        .iter()
        .filter(|c| c.blank)
        .count();
    PlacedLine {
        clusters: range,
        glyphs,
        content_width,
        rtl: paragraph.base_rtl,
        blank_count,
        trailing_blanks: trailing,
        last_in_paragraph,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Left,
    Right,
    Center,
}

pub(crate) fn alignment_offset(line: &PlacedLine, align: TextAlign, container: f32) -> f32 {
    let side = match (align, line.rtl) {
        (TextAlign::Left, _) => Side::Left,
        (TextAlign::Right, _) => Side::Right,
        (TextAlign::Center, _) => Side::Center,
        (TextAlign::Start | TextAlign::Justify, false) | (TextAlign::End, true) => Side::Left,
        (TextAlign::Start | TextAlign::Justify, true) | (TextAlign::End, false) => Side::Right,
    };
    let free = (container - line.content_width).max(0.0);
    match side {
        Side::Left => 0.0,
        Side::Right => free,
        Side::Center => free / 2.0,
    }
}

pub(crate) fn justify(line: &mut PlacedLine, paragraph: &ShapedParagraph, container: f32) -> bool {
    if line.last_in_paragraph || line.blank_count == 0 || line.content_width >= container {
        return false;
    }
    let extra = (container - line.content_width) / line.blank_count as f32;
    let mut shift = 0.0f32;
    let mut content = 0.0f32;
    for placed in &mut line.glyphs {
        placed.x += shift;
        let cluster = &paragraph.clusters[placed.cluster];
        let last_glyph = placed.glyph + 1 == cluster.glyphs.end;
        let trailing_blank = placed.cluster >= line.clusters.end - line.trailing_blanks;
        if cluster.blank && last_glyph && !trailing_blank {
            placed.advance += extra;
            shift += extra;
        }
        content = content.max(placed.x + placed.advance);
    }
    line.content_width = content.max(container);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster(start: usize, advance: f32, blank: bool) -> Cluster {
        Cluster {
            start,
            end: start + 1,
            advance,
            run: 0,
            glyphs: 0..1,
            level: 0,
            blank,
            tab: false,
        }
    }

    fn word_clusters(text: &str, width: f32) -> (Vec<Cluster>, Vec<usize>) {
        let clusters: Vec<_> = text
            .char_indices()
            .map(|(index, ch)| cluster(index, width, ch == ' '))
            .collect();
        (clusters, break_opportunities(text, 0))
    }

    #[test]
    fn word_wrap_breaks_after_spaces_and_hangs_blanks() {
        let (clusters, breaks) = word_clusters("aa bb cc", 10.0);
        let lines = break_lines(&clusters, &breaks, WrapMode::Word, Some(55.0));
        assert_eq!(lines, vec![0..6, 6..8]);
    }

    #[test]
    fn word_mode_never_splits_a_word() {
        let (clusters, breaks) = word_clusters("abcdefgh ij", 10.0);
        let lines = break_lines(&clusters, &breaks, WrapMode::Word, Some(30.0));
        assert_eq!(lines, vec![0..9, 9..11]);
    }

    #[test]
    fn word_or_glyph_splits_overlong_words_only() {
        let (clusters, breaks) = word_clusters("abcdefgh ij", 10.0);
        let lines = break_lines(&clusters, &breaks, WrapMode::WordOrGlyph, Some(30.0));
        assert_eq!(lines, vec![0..3, 3..6, 6..9, 9..11]);
    }

    #[test]
    fn glyph_mode_fills_every_line() {
        let (clusters, breaks) = word_clusters("ab cd ef", 10.0);
        let lines = break_lines(&clusters, &breaks, WrapMode::Glyph, Some(40.0));
        assert_eq!(lines, vec![0..4, 4..8]);
    }

    #[test]
    fn no_wrap_or_no_limit_is_one_line() {
        let (clusters, breaks) = word_clusters("ab cd ef", 10.0);
        assert_eq!(
            break_lines(&clusters, &breaks, WrapMode::None, Some(5.0)),
            vec![0..8]
        );
        assert_eq!(
            break_lines(&clusters, &breaks, WrapMode::Word, None),
            vec![0..8]
        );
    }

    #[test]
    fn tabs_advance_to_the_next_stop() {
        let mut tab = cluster(0, 80.0, false);
        tab.tab = true;
        assert_eq!(tab_aware_width(&tab, 0.0), 80.0);
        assert_eq!(tab_aware_width(&tab, 30.0), 50.0);
        assert_eq!(tab_aware_width(&tab, 80.0), 80.0);
    }
}
