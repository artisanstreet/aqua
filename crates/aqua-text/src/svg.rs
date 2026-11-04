use std::borrow::Cow;
use std::fmt::Write;

use roxmltree::{Document, Node, NodeType};

use crate::font::FontManager;
use crate::layout::{LaidOut, TextLayout};
use crate::raster::raster_key;
use crate::slug::{em_outline, PathCommand};
use crate::style::{FontFamily, FontSlant, FontWeight, LineMode, TextStyle, WrapMode};

const TEXT_ONLY_ATTRIBUTES: &[&str] = &[
    "x",
    "y",
    "dx",
    "dy",
    "rotate",
    "textLength",
    "lengthAdjust",
    "style",
    "id",
    "class",
];

#[derive(Clone, Debug, PartialEq)]
struct Face {
    families: Vec<FontFamily>,
    size_px: f32,
    weight: FontWeight,
    slant: FontSlant,
    letter_spacing_px: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Anchor {
    Start,
    Middle,
    End,
}

#[derive(Debug)]
struct Chunk {
    text: String,
    face: Face,
    anchor: Anchor,
    x: Option<f32>,
    y: Option<f32>,
    dx: f32,
    dy: f32,
    paint: Vec<(String, String)>,
}

fn declarations(node: Node<'_, '_>) -> Vec<(String, String)> {
    node.attribute("style")
        .map(|style| {
            style
                .split(';')
                .filter_map(|declaration| declaration.split_once(':'))
                .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn property(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.ancestors()
        .filter(Node::is_element)
        .find_map(|ancestor| own_property(ancestor, name))
}

fn own_property(node: Node<'_, '_>, name: &str) -> Option<String> {
    declarations(node)
        .into_iter()
        .rev()
        .find(|(declared, _)| declared == name)
        .map(|(_, value)| value)
        .or_else(|| node.attribute(name).map(|value| value.trim().to_string()))
}

fn leading_number(value: &str) -> Option<f32> {
    let end = value
        .char_indices()
        .find(|&(index, c)| {
            !(c.is_ascii_digit() || c == '.' || (index == 0 && (c == '-' || c == '+')))
        })
        .map_or(value.len(), |(index, _)| index);
    value[..end].parse().ok()
}

fn number_list(value: Option<&str>) -> Vec<f32> {
    value
        .map(|value| {
            value
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|part| !part.is_empty())
                .filter_map(leading_number)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_family(value: &str) -> Vec<FontFamily> {
    value
        .split(',')
        .map(|name| name.trim().trim_matches(|c| c == '"' || c == '\'').trim())
        .filter(|name| !name.is_empty())
        .map(|name| match name.to_ascii_lowercase().as_str() {
            "sans-serif" | "system-ui" => FontFamily::SansSerif,
            "serif" => FontFamily::Serif,
            "monospace" => FontFamily::Monospace,
            "cursive" => FontFamily::Cursive,
            "fantasy" => FontFamily::Fantasy,
            _ => FontFamily::Named(name.to_string()),
        })
        .collect()
}

fn parse_weight(value: &str, inherited: FontWeight) -> FontWeight {
    match value {
        "normal" => FontWeight::NORMAL,
        "bold" => FontWeight::BOLD,
        "bolder" => FontWeight((inherited.0 + 300).min(900)),
        "lighter" => FontWeight(inherited.0.saturating_sub(300).max(100)),
        other => leading_number(other)
            .map(|weight| FontWeight(weight.clamp(1.0, 1000.0) as u16))
            .unwrap_or(inherited),
    }
}

fn face_of(node: Node<'_, '_>) -> Face {
    let size_px = property(node, "font-size")
        .and_then(|value| leading_number(&value))
        .filter(|size| *size > 0.0)
        .unwrap_or(16.0);
    Face {
        families: property(node, "font-family")
            .map(|value| parse_family(&value))
            .filter(|families| !families.is_empty())
            .unwrap_or_else(|| vec![FontFamily::SansSerif]),
        size_px,
        weight: property(node, "font-weight")
            .map(|value| parse_weight(&value, FontWeight::NORMAL))
            .unwrap_or(FontWeight::NORMAL),
        slant: match property(node, "font-style").as_deref() {
            Some("italic") => FontSlant::Italic,
            Some("oblique") => FontSlant::Oblique,
            _ => FontSlant::Normal,
        },
        letter_spacing_px: property(node, "letter-spacing")
            .and_then(|value| leading_number(&value))
            .unwrap_or(0.0),
    }
}

fn anchor_of(node: Node<'_, '_>) -> Anchor {
    match property(node, "text-anchor").as_deref() {
        Some("middle") => Anchor::Middle,
        Some("end") => Anchor::End,
        _ => Anchor::Start,
    }
}

fn paint_of(node: Node<'_, '_>) -> Vec<(String, String)> {
    [
        "fill",
        "fill-opacity",
        "stroke",
        "stroke-width",
        "stroke-opacity",
        "opacity",
    ]
    .into_iter()
    .filter_map(|name| own_property(node, name).map(|value| (name.to_string(), value)))
    .collect()
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn spaced(raw: &str) -> String {
    let body = collapse(raw);
    match (
        body.is_empty(),
        raw.starts_with(char::is_whitespace),
        raw.ends_with(char::is_whitespace),
    ) {
        (true, true, _) => " ".to_string(),
        (true, false, _) => String::new(),
        (false, lead, trail) => format!(
            "{}{body}{}",
            if lead { " " } else { "" },
            if trail { " " } else { "" }
        ),
    }
}

fn first_of(node: Node<'_, '_>, name: &str) -> Option<f32> {
    number_list(node.attribute(name)).first().copied()
}

fn chunk_from(text: Node<'_, '_>, owner: Node<'_, '_>, raw: &str) -> Chunk {
    let mut paint = paint_of(text);
    if owner != text {
        for (name, value) in paint_of(owner) {
            paint.retain(|(existing, _)| *existing != name);
            paint.push((name, value));
        }
    }
    Chunk {
        text: spaced(raw),
        face: face_of(owner),
        anchor: anchor_of(owner),
        x: None,
        y: None,
        dx: 0.0,
        dy: 0.0,
        paint,
    }
}

fn chunks_of(text: Node<'_, '_>) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut push = |mut chunk: Chunk, owner: Node<'_, '_>, head: bool| {
        if chunks.last().is_some_and(|last| last.text.ends_with(' ')) {
            chunk.text = chunk.text.trim_start().to_string();
        }
        if chunk.text.is_empty() {
            return;
        }
        let inherit = chunks.is_empty();
        let pick = |name: &str| {
            first_of(owner, name).or_else(|| {
                (inherit && owner != text)
                    .then(|| first_of(text, name))
                    .flatten()
            })
        };
        if head || inherit {
            chunk.x = pick("x");
            chunk.y = pick("y");
            chunk.dx = pick("dx").unwrap_or(0.0);
            chunk.dy = pick("dy").unwrap_or(0.0);
        }
        chunks.push(chunk);
    };
    for child in text.children() {
        match child.node_type() {
            NodeType::Text => {
                let chunk = chunk_from(text, text, child.text().unwrap_or_default());
                push(chunk, text, false);
            }
            NodeType::Element if child.tag_name().name() == "tspan" => {
                let mut head = true;
                for inner in child.children().filter(|n| n.node_type() == NodeType::Text) {
                    let chunk = chunk_from(text, child, inner.text().unwrap_or_default());
                    push(chunk, child, head);
                    head = false;
                }
            }
            _ => {}
        }
    }
    if let Some(first) = chunks.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = chunks.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    chunks.retain(|chunk| !chunk.text.is_empty());
    chunks
}

fn style_of(face: &Face) -> TextStyle {
    TextStyle {
        families: face.families.clone(),
        size_px: face.size_px,
        line_height_px: face.size_px * 1.2,
        weight: face.weight,
        slant: face.slant,
        letter_spacing_em: (face.letter_spacing_px != 0.0)
            .then(|| face.letter_spacing_px / face.size_px),
        ..TextStyle::default()
    }
}

struct Placed {
    laid: std::sync::Arc<LaidOut>,
    width: f32,
}

fn lay_out(fonts: &mut FontManager, chunk: &Chunk) -> Placed {
    let mut layout = TextLayout::new();
    layout.set_wrap(WrapMode::None);
    layout.set_line_mode(LineMode::Single);
    layout.set_text(&chunk.text, &style_of(&chunk.face));
    let laid = layout.layout(fonts);
    let width = laid.lines.first().map_or(0.0, |line| line.width);
    Placed { laid, width }
}

fn format_commands(out: &mut String, commands: &[PathCommand], origin: [f32; 2], size: f32) {
    let map = |point: [f32; 2]| (origin[0] + point[0] * size, origin[1] - point[1] * size);
    for command in commands {
        let _ = match *command {
            PathCommand::Move(p) => {
                let (x, y) = map(p);
                write!(out, "M{x:.3} {y:.3}")
            }
            PathCommand::Line(p) => {
                let (x, y) = map(p);
                write!(out, "L{x:.3} {y:.3}")
            }
            PathCommand::Quad(c, p) => {
                let ((cx, cy), (x, y)) = (map(c), map(p));
                write!(out, "Q{cx:.3} {cy:.3} {x:.3} {y:.3}")
            }
            PathCommand::Cubic(c0, c1, p) => {
                let ((ax, ay), (bx, by), (x, y)) = (map(c0), map(c1), map(p));
                write!(out, "C{ax:.3} {ay:.3} {bx:.3} {by:.3} {x:.3} {y:.3}")
            }
            PathCommand::Close => write!(out, "Z"),
        };
    }
}

fn outline_run(fonts: &mut FontManager, placed: &Placed, origin: [f32; 2]) -> String {
    let mut data = String::new();
    for glyph in placed.laid.lines.iter().flat_map(|line| line.glyphs.iter()) {
        let key = raster_key(glyph, 0);
        if let Some(commands) = em_outline(fonts, &key) {
            let glyph_origin = [
                origin[0] + glyph.x + glyph.x_offset,
                origin[1] + glyph.y - glyph.y_offset,
            ];
            format_commands(&mut data, &commands, glyph_origin, glyph.font_size_px);
        }
    }
    data
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn emit(text: Node<'_, '_>, prefix: &str, chunk_paint: &[(String, String)], data: &str) -> String {
    let mut out = format!("<{prefix}path d=\"{data}\"");
    let mut attributes: Vec<(String, String)> = text
        .attributes()
        .filter(|attribute| {
            let name = attribute.name();
            !name.starts_with("font-")
                && !name.starts_with("text-")
                && !name.ends_with("-spacing")
                && !TEXT_ONLY_ATTRIBUTES.contains(&name)
                && attribute.namespace().is_none()
        })
        .map(|attribute| (attribute.name().to_string(), attribute.value().to_string()))
        .collect();
    for (name, value) in chunk_paint {
        attributes.retain(|(existing, _)| existing != name);
        attributes.push((name.clone(), value.clone()));
    }
    if !attributes.iter().any(|(name, _)| name == "fill") && own_property(text, "fill").is_none() {
        attributes.push(("fill".to_string(), "#000000".to_string()));
    }
    for (name, value) in attributes {
        let _ = write!(out, " {name}=\"{}\"", escape(&value));
    }
    out.push_str("/>");
    out
}

fn replacement(fonts: &mut FontManager, text: Node<'_, '_>) -> String {
    let prefix = text
        .tag_name()
        .namespace()
        .and_then(|namespace| text.lookup_prefix(namespace))
        .filter(|prefix| !prefix.is_empty())
        .map(|prefix| format!("{prefix}:"))
        .unwrap_or_default();
    let chunks = chunks_of(text);
    let placed: Vec<Placed> = chunks.iter().map(|chunk| lay_out(fonts, chunk)).collect();
    let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        match (chunk.x.is_some() || index == 0, groups.last_mut()) {
            (false, Some(group)) => group.end = index + 1,
            _ => groups.push(index..index + 1),
        }
    }
    let mut pen = [0.0f32, 0.0f32];
    let mut out = String::new();
    for group in groups {
        let total: f32 = placed[group.clone()].iter().map(|run| run.width).sum();
        let shift = match chunks[group.start].anchor {
            Anchor::Start => 0.0,
            Anchor::Middle => -total / 2.0,
            Anchor::End => -total,
        };
        for index in group {
            let chunk = &chunks[index];
            pen = [
                chunk.x.unwrap_or(pen[0]) + chunk.dx,
                chunk.y.unwrap_or(pen[1]) + chunk.dy,
            ];
            let data = outline_run(fonts, &placed[index], [pen[0] + shift, pen[1]]);
            pen[0] += placed[index].width;
            if !data.is_empty() {
                out.push_str(&emit(text, &prefix, &chunk.paint, &data));
            }
        }
    }
    out
}

pub fn outline_text<'a>(svg: &'a [u8], fonts: &mut FontManager) -> Cow<'a, [u8]> {
    let Ok(source) = std::str::from_utf8(svg) else {
        return Cow::Borrowed(svg);
    };
    if !source.contains("<") || !source.contains("text") {
        return Cow::Borrowed(svg);
    }
    let Ok(document) = Document::parse(source) else {
        return Cow::Borrowed(svg);
    };
    let texts: Vec<Node<'_, '_>> = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "text")
        .filter(|node| {
            !node
                .ancestors()
                .skip(1)
                .any(|ancestor| ancestor.is_element() && ancestor.tag_name().name() == "text")
        })
        .collect();
    if texts.is_empty() {
        return Cow::Borrowed(svg);
    }
    let mut rewritten = String::with_capacity(source.len());
    let mut cursor = 0;
    for node in texts {
        let range = node.range();
        rewritten.push_str(&source[cursor..range.start]);
        rewritten.push_str(&replacement(fonts, node));
        cursor = range.end;
    }
    rewritten.push_str(&source[cursor..]);
    Cow::Owned(rewritten.into_bytes())
}
