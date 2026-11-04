use std::ops::Range;
use std::sync::Arc;

use unicode_bidi::ParagraphBidiInfo;
use unicode_script::Script;

use crate::font::{FontEntry, FontManager};
use crate::raster::embolden_width;
use crate::style::{StyledSpan, TextStyle};

pub(crate) struct ShapeParams<'a> {
    pub size_px: f32,
    pub rtl: bool,
    pub features: &'a [harfrust::Feature],
    pub letter_spacing_px: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ShapedGlyph {
    pub glyph_id: u16,
    pub cluster: usize,
    pub advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

fn harfrust_script(script: Script) -> Option<harfrust::Script> {
    let name = script.short_name();
    let bytes: [u8; 4] = name.as_bytes().try_into().ok()?;
    harfrust::Script::from_iso15924_tag(harfrust::Tag::new(&bytes))
}

pub(crate) fn text_script(text: &str) -> Option<Script> {
    text.chars()
        .map(Script::from)
        .find(|script| !matches!(script, Script::Common | Script::Inherited | Script::Unknown))
}

pub(crate) fn shape_run(
    fonts: &mut FontManager,
    entry: &FontEntry,
    text: &str,
    params: &ShapeParams<'_>,
) -> Vec<ShapedGlyph> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut buffer = fonts.take_buffer();
    buffer.push_str(text);
    buffer.set_direction(if params.rtl {
        harfrust::Direction::RightToLeft
    } else {
        harfrust::Direction::LeftToRight
    });
    if let Some(script) = text_script(text).and_then(harfrust_script) {
        buffer.set_script(script);
    }
    buffer.guess_segment_properties();
    buffer.set_flags(harfrust::BufferFlags::BEGINNING_OF_TEXT | harfrust::BufferFlags::END_OF_TEXT);
    let scale = params.size_px / entry.units_per_em();
    let widening = match entry.synthetic_bold() {
        true => embolden_width(params.size_px),
        false => 0.0,
    };
    let shaped = entry.with_shaper(|shaper| {
        let glyphs = shaper.shape(
            buffer,
            harfrust::ShapeOptions::new().features(params.features),
        );
        let mut out = Vec::with_capacity(glyphs.len());
        for (info, position) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
            out.push(ShapedGlyph {
                glyph_id: info.glyph_id as u16,
                cluster: info.cluster as usize,
                advance: match position.x_advance {
                    0 => 0.0,
                    advance => advance as f32 * scale + widening,
                },
                x_offset: position.x_offset as f32 * scale,
                y_offset: position.y_offset as f32 * scale,
            });
        }
        (out, glyphs.clear())
    });
    let Some((mut glyphs, buffer)) = shaped else {
        return Vec::new();
    };
    fonts.return_buffer(buffer);
    if params.letter_spacing_px != 0.0 {
        let count = glyphs.len();
        for index in 0..count {
            let cluster_ends =
                index + 1 == count || glyphs[index + 1].cluster != glyphs[index].cluster;
            if cluster_ends {
                glyphs[index].advance += params.letter_spacing_px;
            }
        }
    }
    glyphs
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RunGlyph {
    pub glyph_id: u16,
    pub advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

pub(crate) struct ShapedRun {
    pub entry: Option<Arc<FontEntry>>,
    pub size_px: f32,
    pub line_height: f32,
    pub glyphs: Vec<RunGlyph>,
}

#[derive(Clone, Debug)]
pub(crate) struct Cluster {
    pub start: usize,
    pub end: usize,
    pub advance: f32,
    pub run: usize,
    pub glyphs: Range<usize>,
    pub level: u8,
    pub blank: bool,
    pub tab: bool,
}

pub(crate) struct ShapedParagraph {
    pub start: usize,
    pub end: usize,
    pub base_rtl: bool,
    pub clusters: Vec<Cluster>,
    pub runs: Vec<ShapedRun>,
    pub empty_line_height: f32,
    pub empty_ascent: f32,
    pub empty_descent: f32,
    pub breaks: std::sync::OnceLock<Vec<usize>>,
}

pub(crate) struct SpanText {
    pub range: Range<usize>,
}

pub(crate) struct ShapeContext<'a> {
    pub spans: &'a [StyledSpan],
    pub span_ranges: &'a [SpanText],
    pub scale: f32,
    pub gpui_line_height: bool,
    pub inline_cells: bool,
}

pub(crate) fn scaled_line_height(height: f32, scale: f32, gpui: bool) -> f32 {
    let height = height * scale;
    if gpui {
        (height.abs() - 0.5).ceil().copysign(height).max(1.0)
    } else {
        height.max(1.0)
    }
}

fn is_hanging_blank(grapheme: &str) -> bool {
    grapheme
        .chars()
        .all(|ch| ch.is_whitespace() && !matches!(ch, '\u{00a0}' | '\u{202f}' | '\u{2007}'))
}

fn style_features(
    style: &TextStyle,
    fallback: bool,
    letter_spacing: bool,
) -> Vec<harfrust::Feature> {
    let mut features = Vec::new();
    if fallback && style.families.len() <= 1 {
        return features;
    }
    if letter_spacing {
        for tag in [b"liga", b"clig"] {
            features.push(harfrust::Feature::new(harfrust::Tag::new(tag), 0, ..));
        }
    }
    for feature in &style.font_features {
        features.push(harfrust::Feature::new(
            harfrust::Tag::new(&feature.tag),
            feature.value,
            ..,
        ));
    }
    features
}

struct Piece {
    span: usize,
    range: Range<usize>,
    entry: Option<Arc<FontEntry>>,
    fallback: bool,
    level: u8,
}

fn is_real_script(script: Script) -> bool {
    !matches!(script, Script::Common | Script::Inherited | Script::Unknown)
}

fn split_pieces(
    fonts: &mut FontManager,
    context: &ShapeContext<'_>,
    display: &str,
    range: &Range<usize>,
    level_at: &dyn Fn(usize) -> u8,
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for (index, span) in context.spans.iter().enumerate() {
        let span_range = &context.span_ranges[index].range;
        let from = span_range.start.max(range.start);
        let to = span_range.end.min(range.end);
        if from >= to {
            continue;
        }
        for (font_range, entry, fallback) in fonts.font_runs(&display[from..to], &span.style) {
            let font_from = from + font_range.start;
            let font_to = from + font_range.end;
            let mut start = font_from;
            let mut current_level = level_at(font_from - range.start);
            let mut current_script: Option<Script> = None;
            for (offset, character) in display[font_from..font_to].char_indices() {
                let position = font_from + offset;
                let level = level_at(position - range.start);
                let script = Script::from(character);
                let script_changes =
                    is_real_script(script) && current_script.is_some_and(|known| known != script);
                if position > start && (level != current_level || script_changes) {
                    pieces.push(Piece {
                        span: index,
                        range: start..position,
                        entry: entry.clone(),
                        fallback,
                        level: current_level,
                    });
                    start = position;
                    current_level = level;
                    current_script = None;
                }
                if is_real_script(script) {
                    current_script = Some(script);
                }
            }
            if start < font_to {
                pieces.push(Piece {
                    span: index,
                    range: start..font_to,
                    entry,
                    fallback,
                    level: current_level,
                });
            }
        }
    }
    pieces
}

pub(crate) fn shape_paragraph(
    fonts: &mut FontManager,
    context: &ShapeContext<'_>,
    display: &str,
    range: Range<usize>,
) -> ShapedParagraph {
    let text = &display[range.clone()];
    let bidi: Option<(Vec<u8>, bool)> = text
        .chars()
        .any(|ch| ch >= '\u{0590}')
        .then(|| ParagraphBidiInfo::new(text, None))
        .map(|info| {
            (
                info.levels.iter().map(|level| level.number()).collect(),
                info.paragraph_level.is_rtl(),
            )
        });
    let base_rtl = bidi.as_ref().is_some_and(|(_, rtl)| *rtl);
    let level_at = |offset: usize| -> u8 {
        bidi.as_ref()
            .map_or(0, |(levels, _)| levels.get(offset).copied().unwrap_or(0))
    };

    let pieces = split_pieces(fonts, context, display, &range, &level_at);
    let mut runs: Vec<ShapedRun> = Vec::new();
    let mut clusters: Vec<Cluster> = Vec::new();
    for piece in pieces {
        let Some(entry) = piece.entry.clone() else {
            continue;
        };
        let span = &context.spans[piece.span];
        let size_px = (span.style.size_px * context.scale).max(1.0);
        let line_height = scaled_line_height(
            span.style.line_height_px,
            context.scale,
            context.gpui_line_height,
        );
        let piece_text = &display[piece.range.clone()];
        let rtl = piece.level % 2 == 1;
        let tracking = span.style.letter_spacing_em.unwrap_or(0.0) * size_px;
        let features = style_features(&span.style, piece.fallback, tracking != 0.0);
        let mut spacing = tracking;
        if let Some(width) = span.inline_cell_width().filter(|_| context.inline_cells) {
            let natural = shape_run(
                fonts,
                &entry,
                piece_text,
                &ShapeParams {
                    size_px,
                    rtl,
                    features: &features,
                    letter_spacing_px: 0.0,
                },
            );
            let natural_width: f32 = natural.iter().map(|glyph| glyph.advance).sum();
            spacing =
                (width * context.scale - natural_width).max(0.0) / natural.len().max(1) as f32;
        }
        let shaped = shape_run(
            fonts,
            &entry,
            piece_text,
            &ShapeParams {
                size_px,
                rtl,
                features: &features,
                letter_spacing_px: spacing,
            },
        );
        let mut groups: Vec<&[ShapedGlyph]> = Vec::new();
        let mut group_start = 0;
        for index in 1..=shaped.len() {
            if index == shaped.len() || shaped[index].cluster != shaped[group_start].cluster {
                groups.push(&shaped[group_start..index]);
                group_start = index;
            }
        }
        if rtl {
            groups.reverse();
        }
        let run_index = runs.len();
        let mut run_glyphs: Vec<RunGlyph> = Vec::with_capacity(shaped.len());
        for (position, group) in groups.iter().enumerate() {
            let start = if position == 0 {
                piece.range.start
            } else {
                piece.range.start + group[0].cluster
            };
            let end = groups
                .get(position + 1)
                .map_or(piece.range.end, |next| piece.range.start + next[0].cluster);
            let slice = &display[start..end];
            let first = run_glyphs.len();
            run_glyphs.extend(group.iter().map(|glyph| RunGlyph {
                glyph_id: glyph.glyph_id,
                advance: glyph.advance,
                x_offset: glyph.x_offset,
                y_offset: glyph.y_offset,
            }));
            let tab = slice == "\t";
            let advance = if tab {
                8.0 * entry.space_em() * size_px
            } else {
                group.iter().map(|glyph| glyph.advance).sum()
            };
            clusters.push(Cluster {
                start,
                end,
                advance,
                run: run_index,
                glyphs: first..run_glyphs.len(),
                level: piece.level,
                blank: is_hanging_blank(slice),
                tab,
            });
        }
        runs.push(ShapedRun {
            entry: Some(entry),
            size_px,
            line_height,
            glyphs: run_glyphs,
        });
    }
    let empty_span = context
        .span_ranges
        .iter()
        .position(|span| span.range.end > range.start)
        .unwrap_or(context.spans.len().saturating_sub(1));
    let empty_line_height = context.spans.get(empty_span).map_or(
        scaled_line_height(22.0, context.scale, context.gpui_line_height),
        |span| {
            scaled_line_height(
                span.style.line_height_px,
                context.scale,
                context.gpui_line_height,
            )
        },
    );
    let (empty_ascent, empty_descent) = context
        .spans
        .get(empty_span)
        .and_then(|span| {
            fonts.primary(&span.style).map(|entry| {
                let size = (span.style.size_px * context.scale).max(1.0);
                (entry.ascent_em() * size, entry.descent_em() * size)
            })
        })
        .unwrap_or((0.0, 0.0));
    ShapedParagraph {
        start: range.start,
        end: range.end,
        base_rtl,
        clusters,
        runs,
        empty_line_height,
        empty_ascent,
        empty_descent,
        breaks: std::sync::OnceLock::new(),
    }
}
