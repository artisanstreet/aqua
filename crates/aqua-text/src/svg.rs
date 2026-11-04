use std::borrow::Cow;
use std::fmt::Write;
use std::sync::Arc;

use roxmltree::{Document, Node, NodeType};

use crate::font::FontManager;
use crate::layout::{LaidOut, TextLayout};
use crate::raster::raster_key;
use crate::slug::{em_outline, PathCommand};
use crate::style::{FontFamily, FontSlant, FontWeight, LineMode, TextStyle, WrapMode};

pub mod length;
pub mod path;
pub mod sheet;

use length::{font_size, lengths, parse_length, resolve, Length, MEDIUM_FONT_PX};
use path::Polyline;
use sheet::Sheet;

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

#[derive(Clone, Copy, Debug, PartialEq)]
enum Offset {
    Px(f32),
    Fraction(f32),
}

#[derive(Clone, Debug)]
struct OnPath {
    line: Arc<Polyline>,
    offset: Offset,
}

#[derive(Debug)]
struct Chunk {
    on_path: Option<OnPath>,
    text: String,
    face: Face,
    anchor: Anchor,
    x: Option<f32>,
    y: Option<f32>,
    dx: f32,
    dy: f32,
    paint: Vec<(String, String)>,
}

fn declarations(node: Node<'_, '_>, sheet: &Sheet) -> Vec<(String, String)> {
    let mut all = sheet.declarations(node);
    all.extend(
        node.attribute("style")
            .map(|style| {
                style
                    .split(';')
                    .filter_map(|declaration| declaration.split_once(':'))
                    .map(|(name, value)| {
                        (
                            name.trim().to_string(),
                            value.replace("!important", "").trim().to_string(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    all
}

fn property(node: Node<'_, '_>, sheet: &Sheet, name: &str) -> Option<String> {
    node.ancestors()
        .filter(Node::is_element)
        .find_map(|ancestor| own_property(ancestor, sheet, name))
}

fn own_property(node: Node<'_, '_>, sheet: &Sheet, name: &str) -> Option<String> {
    declarations(node, sheet)
        .into_iter()
        .rev()
        .find(|(declared, _)| declared == name)
        .map(|(_, value)| value)
        .or_else(|| node.attribute(name).map(|value| value.trim().to_string()))
}

fn font_px(node: Node<'_, '_>, sheet: &Sheet) -> f32 {
    let mut chain: Vec<Node<'_, '_>> = node.ancestors().filter(Node::is_element).collect();
    chain.reverse();
    chain.into_iter().fold(MEDIUM_FONT_PX, |parent, element| {
        own_property(element, sheet, "font-size")
            .and_then(|value| font_size(&value, parent))
            .unwrap_or(parent)
    })
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

fn face_of(node: Node<'_, '_>, sheet: &Sheet) -> Face {
    let size_px = font_px(node, sheet);
    Face {
        families: property(node, sheet, "font-family")
            .map(|value| parse_family(&value))
            .filter(|families| !families.is_empty())
            .unwrap_or_else(|| vec![FontFamily::SansSerif]),
        size_px,
        weight: property(node, sheet, "font-weight")
            .map(|value| parse_weight(&value, FontWeight::NORMAL))
            .unwrap_or(FontWeight::NORMAL),
        slant: match property(node, sheet, "font-style").as_deref() {
            Some("italic") => FontSlant::Italic,
            Some("oblique") => FontSlant::Oblique,
            _ => FontSlant::Normal,
        },
        letter_spacing_px: property(node, sheet, "letter-spacing")
            .and_then(|value| parse_length(&value))
            .and_then(|length| resolve(length, size_px))
            .unwrap_or(0.0),
    }
}

fn anchor_of(node: Node<'_, '_>, sheet: &Sheet) -> Anchor {
    match property(node, sheet, "text-anchor").as_deref() {
        Some("middle") => Anchor::Middle,
        Some("end") => Anchor::End,
        _ => Anchor::Start,
    }
}

fn paint_of(node: Node<'_, '_>, sheet: &Sheet) -> Vec<(String, String)> {
    [
        "fill",
        "fill-opacity",
        "stroke",
        "stroke-width",
        "stroke-opacity",
        "opacity",
    ]
    .into_iter()
    .filter_map(|name| own_property(node, sheet, name).map(|value| (name.to_string(), value)))
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

fn first_of(node: Node<'_, '_>, sheet: &Sheet, name: &str) -> Option<f32> {
    lengths(node.attribute(name), font_px(node, sheet))
        .first()
        .copied()
        .flatten()
}

struct Collector<'a, 'd, 'i> {
    document: &'a Document<'d>,
    text: Node<'d, 'i>,
    sheet: &'a Sheet,
    chunks: Vec<Chunk>,
}

fn resolve_path(document: &Document<'_>, node: Node<'_, '_>) -> Option<Arc<Polyline>> {
    let reference = node
        .attribute(("http://www.w3.org/1999/xlink", "href"))
        .or_else(|| node.attribute("href"))?;
    let id = reference.strip_prefix('#')?;
    let target = document
        .descendants()
        .find(|candidate| candidate.is_element() && candidate.attribute("id") == Some(id))?;
    (target.tag_name().name() == "path")
        .then(|| target.attribute("d"))
        .flatten()
        .and_then(Polyline::parse)
        .map(Arc::new)
}

fn start_offset(node: Node<'_, '_>, sheet: &Sheet) -> Offset {
    node.attribute("startOffset")
        .and_then(parse_length)
        .map_or(Offset::Px(0.0), |length| match length {
            Length::Percent(fraction) => Offset::Fraction(fraction),
            other => Offset::Px(resolve(other, font_px(node, sheet)).unwrap_or(0.0)),
        })
}

impl<'a, 'd, 'i> Collector<'a, 'd, 'i> {
    fn chunk_from(&self, owner: Node<'d, 'i>, raw: &str, on_path: &Option<OnPath>) -> Chunk {
        let mut paint = paint_of(self.text, self.sheet);
        if owner != self.text {
            for (name, value) in paint_of(owner, self.sheet) {
                paint.retain(|(existing, _)| *existing != name);
                paint.push((name, value));
            }
        }
        Chunk {
            on_path: on_path.clone(),
            text: spaced(raw),
            face: face_of(owner, self.sheet),
            anchor: anchor_of(owner, self.sheet),
            x: None,
            y: None,
            dx: 0.0,
            dy: 0.0,
            paint,
        }
    }

    fn push(&mut self, mut chunk: Chunk, owner: Node<'d, 'i>, head: bool) {
        if self
            .chunks
            .last()
            .is_some_and(|last| last.text.ends_with(' '))
        {
            chunk.text = chunk.text.trim_start().to_string();
        }
        if chunk.text.is_empty() {
            return;
        }
        let inherit = self.chunks.is_empty();
        let sheet = self.sheet;
        let text = self.text;
        let pick = |name: &str| {
            first_of(owner, sheet, name).or_else(|| {
                (inherit && owner != text)
                    .then(|| first_of(text, sheet, name))
                    .flatten()
            })
        };
        if chunk.on_path.is_none() && (head || inherit) {
            chunk.x = pick("x");
            chunk.y = pick("y");
            chunk.dx = pick("dx").unwrap_or(0.0);
            chunk.dy = pick("dy").unwrap_or(0.0);
        }
        self.chunks.push(chunk);
    }

    fn walk(&mut self, parent: Node<'d, 'i>, on_path: Option<OnPath>) {
        for child in parent.children() {
            match (child.node_type(), child.tag_name().name()) {
                (NodeType::Text, _) => {
                    let chunk = self.chunk_from(parent, child.text().unwrap_or_default(), &on_path);
                    self.push(chunk, parent, false);
                }
                (NodeType::Element, "tspan") => {
                    let mut head = true;
                    for inner in child.children().filter(|n| n.node_type() == NodeType::Text) {
                        let chunk =
                            self.chunk_from(child, inner.text().unwrap_or_default(), &on_path);
                        self.push(chunk, child, head);
                        head = false;
                    }
                }
                (NodeType::Element, "textPath") => {
                    let line = resolve_path(self.document, child);
                    let along = line.map(|line| OnPath {
                        line,
                        offset: start_offset(child, self.sheet),
                    });
                    if along.is_some() {
                        self.walk(child, along);
                    }
                }
                _ => {}
            }
        }
    }
}

fn chunks_of(document: &Document<'_>, sheet: &Sheet, text: Node<'_, '_>) -> Vec<Chunk> {
    let mut collector = Collector {
        document,
        text,
        sheet,
        chunks: Vec::new(),
    };
    collector.walk(text, None);
    let mut chunks = collector.chunks;
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

#[derive(Clone, Copy, Debug)]
struct Frame {
    origin: [f32; 2],
    cos: f32,
    sin: f32,
    size: f32,
}

impl Frame {
    fn upright(origin: [f32; 2], size: f32) -> Self {
        Self {
            origin,
            cos: 1.0,
            sin: 0.0,
            size,
        }
    }

    fn map(&self, point: [f32; 2]) -> (f32, f32) {
        let x = point[0] * self.size;
        let y = -point[1] * self.size;
        (
            self.origin[0] + self.cos * x - self.sin * y,
            self.origin[1] + self.sin * x + self.cos * y,
        )
    }
}

fn format_commands(out: &mut String, commands: &[PathCommand], frame: Frame) {
    let map = |point: [f32; 2]| frame.map(point);
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
            format_commands(
                &mut data,
                &commands,
                Frame::upright(glyph_origin, glyph.font_size_px),
            );
        }
    }
    data
}

fn outline_along(
    fonts: &mut FontManager,
    placed: &Placed,
    line: &Polyline,
    start: f32,
) -> String {
    let mut data = String::new();
    for glyph in placed.laid.lines.iter().flat_map(|line| line.glyphs.iter()) {
        let middle = start + glyph.x + glyph.x_offset + glyph.w * 0.5;
        let Some(sample) = line.sample(middle) else {
            continue;
        };
        let key = raster_key(glyph, 0);
        let Some(commands) = em_outline(fonts, &key) else {
            continue;
        };
        let (sin, cos) = sample.angle.sin_cos();
        let lift = glyph.y - glyph.y_offset;
        let half = glyph.w * 0.5;
        let origin = [
            sample.point[0] - cos * half - sin * lift,
            sample.point[1] - sin * half + cos * lift,
        ];
        format_commands(
            &mut data,
            &commands,
            Frame {
                origin,
                cos,
                sin,
                size: glyph.font_size_px,
            },
        );
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

fn emit(
    text: Node<'_, '_>,
    sheet: &Sheet,
    prefix: &str,
    chunk_paint: &[(String, String)],
    data: &str,
) -> String {
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
    if !attributes.iter().any(|(name, _)| name == "fill") && property(text, sheet, "fill").is_none()
    {
        attributes.push(("fill".to_string(), "#000000".to_string()));
    }
    for (name, value) in attributes {
        let _ = write!(out, " {name}=\"{}\"", escape(&value));
    }
    out.push_str("/>");
    out
}

fn replacement(
    fonts: &mut FontManager,
    document: &Document<'_>,
    sheet: &Sheet,
    text: Node<'_, '_>,
) -> String {
    let prefix = text
        .tag_name()
        .namespace()
        .and_then(|namespace| text.lookup_prefix(namespace))
        .filter(|prefix| !prefix.is_empty())
        .map(|prefix| format!("{prefix}:"))
        .unwrap_or_default();
    let chunks = chunks_of(document, sheet, text);
    let placed: Vec<Placed> = chunks.iter().map(|chunk| lay_out(fonts, chunk)).collect();
    let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let continues = match (index.checked_sub(1), &chunk.on_path) {
            (None, _) => false,
            (Some(previous), None) => chunk.x.is_none() && chunks[previous].on_path.is_none(),
            (Some(previous), Some(own)) => chunks[previous]
                .on_path
                .as_ref()
                .is_some_and(|before| Arc::ptr_eq(&before.line, &own.line)),
        };
        match (continues, groups.last_mut()) {
            (true, Some(group)) => group.end = index + 1,
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
        let mut along = chunks[group.start].on_path.as_ref().map(|on_path| {
            shift
                + match on_path.offset {
                    Offset::Px(px) => px,
                    Offset::Fraction(fraction) => fraction * on_path.line.length(),
                }
        });
        for index in group {
            let chunk = &chunks[index];
            let data = match (&chunk.on_path, along.as_mut()) {
                (Some(on_path), Some(cursor)) => {
                    let data = outline_along(fonts, &placed[index], &on_path.line, *cursor);
                    *cursor += placed[index].width;
                    data
                }
                _ => {
                    pen = [
                        chunk.x.unwrap_or(pen[0]) + chunk.dx,
                        chunk.y.unwrap_or(pen[1]) + chunk.dy,
                    ];
                    let data = outline_run(fonts, &placed[index], [pen[0] + shift, pen[1]]);
                    pen[0] += placed[index].width;
                    data
                }
            };
            if !data.is_empty() {
                out.push_str(&emit(text, sheet, &prefix, &chunk.paint, &data));
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
    let sheet = Sheet::parse_document(&document);
    let mut rewritten = String::with_capacity(source.len());
    let mut cursor = 0;
    for node in texts {
        let range = node.range();
        rewritten.push_str(&source[cursor..range.start]);
        rewritten.push_str(&replacement(fonts, &document, &sheet, node));
        cursor = range.end;
    }
    rewritten.push_str(&source[cursor..]);
    Cow::Owned(rewritten.into_bytes())
}
