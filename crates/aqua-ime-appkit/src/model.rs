use aqua_core::{ImeEvent as Composition, Point, Rect};
use aqua_protocol::{ImeCommand, ImeDocument, ImeEvent, ImeGeometry, Utf16Range};

fn utf16_len(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count())
        .expect("invariant: IME text is shorter than four billion UTF-16 units")
}

fn byte_at_utf16(text: &str, units: u32) -> usize {
    let mut counted = 0u32;
    for (byte, ch) in text.char_indices() {
        if counted >= units {
            return byte;
        }
        counted += ch.len_utf16() as u32;
    }
    text.len()
}

fn slice(text: &str, range: Utf16Range) -> &str {
    &text[byte_at_utf16(text, range.start)..byte_at_utf16(text, range.end)]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenMap {
    pub left: f64,
    pub top: f64,
}

impl ScreenMap {
    pub fn to_screen(self, rect: Rect) -> (f64, f64, f64, f64) {
        (
            self.left + f64::from(rect.x),
            self.top - f64::from(rect.y + rect.h),
            f64::from(rect.w),
            f64::from(rect.h),
        )
    }

    pub fn to_view(self, (x, y): (f64, f64)) -> Point {
        Point::new((x - self.left) as f32, (self.top - y) as f32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Install,
    Remove,
    Discard,
    Invalidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inserted {
    Typed,
    Taken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Reconversion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Marked {
    text: String,
    anchor: u32,
    selected: Utf16Range,
    replaced: Option<String>,
}

impl Marked {
    fn len(&self) -> u32 {
        utf16_len(&self.text)
    }

    fn range(&self) -> Utf16Range {
        Utf16Range::new(self.anchor, self.anchor + self.len())
    }

    fn cursor(&self) -> (usize, usize) {
        (
            byte_at_utf16(&self.text, self.selected.start),
            byte_at_utf16(&self.text, self.selected.end),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Input {
    Idle,
    Composing(Marked),
    Ended { echo: String },
}

#[derive(Debug, Clone, PartialEq)]
struct Active {
    document: ImeDocument,
    input: Input,
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Off,
    Active(Active),
}

#[derive(Debug)]
pub struct Session {
    phase: Phase,
    events: Vec<ImeEvent>,
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

fn window_end(document: &ImeDocument) -> u32 {
    document.window_start + utf16_len(&document.text)
}

fn clamp(document: &ImeDocument, range: Utf16Range) -> Utf16Range {
    let end = range.end.min(document.total_len);
    Utf16Range::new(range.start.min(end), end)
}

fn in_window(document: &ImeDocument, range: Utf16Range) -> Option<Utf16Range> {
    (range.start >= document.window_start && range.end <= window_end(document)).then(|| {
        Utf16Range::new(
            range.start - document.window_start,
            range.end - document.window_start,
        )
    })
}

fn replaced(document: &mut ImeDocument, range: Utf16Range, text: &str) {
    if let Some(local) = in_window(document, range) {
        let start = byte_at_utf16(&document.text, local.start);
        let end = byte_at_utf16(&document.text, local.end);
        document.text.replace_range(start..end, text);
    }
    let inserted = utf16_len(text);
    document.total_len = document.total_len.saturating_sub(range.len()) + inserted;
    let caret = range.start + inserted;
    document.selection = Utf16Range::new(caret, caret);
    document.selection_reversed = false;
}

fn first_line(geometry: &ImeGeometry, range: Utf16Range) -> Utf16Range {
    let line = geometry.lines.iter().find(|line| {
        line.stops
            .iter()
            .any(|stop| (range.start..=range.end).contains(&stop.offset))
    });
    let end = line
        .and_then(|line| {
            line.stops
                .iter()
                .map(|stop| stop.offset)
                .filter(|offset| (range.start..=range.end).contains(offset))
                .max()
        })
        .unwrap_or(range.end);
    Utf16Range::new(range.start, end.max(range.start))
}

fn has_area(rect: Rect) -> bool {
    [rect.x, rect.y, rect.w, rect.h]
        .iter()
        .all(|value| value.is_finite())
        && (rect.w > 0.0 || rect.h > 0.0)
}

fn document_offset(marked: Option<&Marked>, offset: u32) -> u32 {
    match marked {
        None => offset,
        Some(marked) if offset <= marked.anchor => offset,
        Some(marked) if offset <= marked.anchor + marked.len() => marked.anchor,
        Some(marked) => offset - marked.len(),
    }
}

impl Active {
    fn marked(&self) -> Option<&Marked> {
        match &self.input {
            Input::Composing(marked) => Some(marked),
            Input::Idle | Input::Ended { .. } => None,
        }
    }

    fn to_document(&self, offset: u32) -> u32 {
        document_offset(self.marked(), offset)
    }

    fn to_virtual(&self, offset: u32) -> u32 {
        match self.marked() {
            Some(marked) if offset > marked.anchor => offset + marked.len(),
            _ => offset,
        }
    }

    fn document_range(&self, range: Utf16Range) -> Utf16Range {
        Utf16Range::new(self.to_document(range.start), self.to_document(range.end))
    }

    fn excerpt(&self) -> (u32, String) {
        let document = &self.document;
        let mut text = document.text.clone();
        if let Some(marked) = self.marked()
            && let Some(local) = in_window(document, Utf16Range::new(marked.anchor, marked.anchor))
        {
            text.insert_str(byte_at_utf16(&text, local.start), &marked.text);
        }
        (document.window_start, text)
    }
}

impl Session {
    pub fn new() -> Session {
        Session {
            phase: Phase::Off,
            events: Vec::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Active(_))
    }

    pub fn is_composing(&self) -> bool {
        matches!(&self.phase, Phase::Active(active) if active.marked().is_some())
    }

    pub fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    pub fn drain(&mut self, out: &mut Vec<ImeEvent>) {
        out.append(&mut self.events);
    }

    fn active(&self) -> Option<&Active> {
        match &self.phase {
            Phase::Active(active) => Some(active),
            Phase::Off => None,
        }
    }

    pub fn command(&mut self, command: ImeCommand) -> Result<Vec<Effect>, Refusal> {
        let phase = std::mem::replace(&mut self.phase, Phase::Off);
        let (phase, effects) = match (command, phase) {
            (ImeCommand::Reconvert, phase) => {
                self.phase = phase;
                return Err(Refusal::Reconversion);
            }
            (ImeCommand::Enable(document), Phase::Off) => (
                Phase::Active(Active {
                    document,
                    input: Input::Idle,
                }),
                vec![Effect::Install, Effect::Invalidate],
            ),
            (ImeCommand::Enable(document), Phase::Active(active)) => {
                let mut effects = Vec::new();
                if active.marked().is_some() {
                    self.events.push(ImeEvent::Composition(Composition::Cancel));
                    effects.push(Effect::Discard);
                }
                effects.push(Effect::Invalidate);
                (
                    Phase::Active(Active {
                        document,
                        input: Input::Idle,
                    }),
                    effects,
                )
            }
            (ImeCommand::Update(_), Phase::Off) => (Phase::Off, Vec::new()),
            (ImeCommand::Update(document), Phase::Active(mut active)) => {
                let moved = active.document.geometry != document.geometry
                    || active.document.area != document.area
                    || active.document.selection != document.selection
                    || active.document.window_start != document.window_start;
                active.document = document;
                let effects = match moved {
                    true => vec![Effect::Invalidate],
                    false => Vec::new(),
                };
                (Phase::Active(active), effects)
            }
            (ImeCommand::Disable, Phase::Off) => (Phase::Off, Vec::new()),
            (ImeCommand::Disable, Phase::Active(active)) => match active.input {
                Input::Composing(marked) => {
                    self.events.push(ImeEvent::Composition(Composition::Commit {
                        text: marked.text,
                    }));
                    (Phase::Off, vec![Effect::Discard, Effect::Remove])
                }
                Input::Idle | Input::Ended { .. } => (Phase::Off, vec![Effect::Remove]),
            },
            (ImeCommand::CommitComposition, Phase::Active(mut active)) => {
                let effects = match std::mem::replace(&mut active.input, Input::Idle) {
                    Input::Composing(marked) => {
                        self.commit(&mut active, marked.anchor, &marked.text);
                        active.input = Input::Ended { echo: marked.text };
                        vec![Effect::Discard]
                    }
                    other => {
                        active.input = other;
                        Vec::new()
                    }
                };
                (Phase::Active(active), effects)
            }
            (ImeCommand::CancelComposition, Phase::Active(mut active)) => {
                let effects = match std::mem::replace(&mut active.input, Input::Idle) {
                    Input::Composing(marked) => {
                        self.events.push(ImeEvent::Composition(Composition::Cancel));
                        if let Some(original) = &marked.replaced {
                            let at = Utf16Range::new(marked.anchor, marked.anchor);
                            self.events.push(ImeEvent::Replace {
                                range: at,
                                text: original.clone(),
                            });
                            replaced(&mut active.document, at, original);
                        }
                        active.input = Input::Ended { echo: marked.text };
                        vec![Effect::Discard]
                    }
                    other => {
                        active.input = other;
                        Vec::new()
                    }
                };
                (Phase::Active(active), effects)
            }
            (ImeCommand::CommitComposition | ImeCommand::CancelComposition, Phase::Off) => {
                (Phase::Off, Vec::new())
            }
        };
        self.phase = phase;
        Ok(effects)
    }

    fn commit(&mut self, active: &mut Active, anchor: u32, text: &str) {
        let event = match text.is_empty() {
            true => Composition::Cancel,
            false => Composition::Commit { text: text.into() },
        };
        self.events.push(ImeEvent::Composition(event));
        replaced(&mut active.document, Utf16Range::new(anchor, anchor), text);
    }

    fn with_active<T>(&mut self, idle: T, change: impl FnOnce(&mut Self, &mut Active) -> T) -> T {
        match std::mem::replace(&mut self.phase, Phase::Off) {
            Phase::Off => idle,
            Phase::Active(mut active) => {
                let result = change(self, &mut active);
                self.phase = Phase::Active(active);
                result
            }
        }
    }

    pub fn key_pressed(&mut self) {
        self.with_active((), |_, active| {
            if let Input::Ended { .. } = active.input {
                active.input = Input::Idle;
            }
        });
    }

    pub fn deactivated(&mut self) {
        self.with_active((), |session, active| {
            if let Input::Composing(marked) = std::mem::replace(&mut active.input, Input::Idle) {
                session.commit(active, marked.anchor, &marked.text);
                active.input = Input::Ended { echo: marked.text };
            }
        });
    }

    pub fn set_marked_text(
        &mut self,
        text: &str,
        selected: Utf16Range,
        replacement: Option<Utf16Range>,
    ) {
        self.with_active((), |session, active| {
            let length = utf16_len(text);
            let selected = Utf16Range::new(
                selected.start.min(length),
                selected.end.min(length).max(selected.start.min(length)),
            );
            match std::mem::replace(&mut active.input, Input::Idle) {
                Input::Composing(_) if text.is_empty() => {
                    session
                        .events
                        .push(ImeEvent::Composition(Composition::Cancel));
                }
                Input::Composing(mut marked) => {
                    marked.text = text.into();
                    marked.selected = selected;
                    session
                        .events
                        .push(ImeEvent::Composition(Composition::Update {
                            marked: marked.text.clone(),
                            cursor: Some(marked.cursor()),
                        }));
                    active.input = Input::Composing(marked);
                }
                Input::Idle | Input::Ended { .. } if text.is_empty() => {}
                Input::Idle | Input::Ended { .. } => {
                    let target = clamp(
                        &active.document,
                        replacement.unwrap_or(active.document.selection),
                    );
                    let original = in_window(&active.document, target)
                        .map(|local| slice(&active.document.text, local).to_owned())
                        .filter(|original| !original.is_empty());
                    if !target.is_empty() {
                        session.events.push(ImeEvent::Replace {
                            range: target,
                            text: String::new(),
                        });
                        replaced(&mut active.document, target, "");
                    }
                    let marked = Marked {
                        text: text.into(),
                        anchor: target.start,
                        selected,
                        replaced: original,
                    };
                    session
                        .events
                        .push(ImeEvent::Composition(Composition::Start));
                    session
                        .events
                        .push(ImeEvent::Composition(Composition::Update {
                            marked: marked.text.clone(),
                            cursor: Some(marked.cursor()),
                        }));
                    active.input = Input::Composing(marked);
                }
            }
        });
    }

    pub fn unmark_text(&mut self) {
        self.with_active((), |session, active| {
            if let Input::Composing(marked) = std::mem::replace(&mut active.input, Input::Idle) {
                session.commit(active, marked.anchor, &marked.text);
            }
        });
    }

    pub fn insert_text(&mut self, text: &str, replacement: Option<Utf16Range>) -> Inserted {
        self.with_active(Inserted::Typed, |session, active| {
            match std::mem::replace(&mut active.input, Input::Idle) {
                Input::Ended { echo } if replacement.is_none() && echo == text => Inserted::Taken,
                Input::Composing(marked) => {
                    match replacement {
                        Some(range) if range != marked.range() => {
                            let target = Utf16Range::new(
                                document_offset(Some(&marked), range.start),
                                document_offset(Some(&marked), range.end),
                            );
                            let target = clamp(&active.document, target);
                            session
                                .events
                                .push(ImeEvent::Composition(Composition::Cancel));
                            session.events.push(ImeEvent::Replace {
                                range: target,
                                text: text.into(),
                            });
                            replaced(&mut active.document, target, text);
                        }
                        _ => session.commit(active, marked.anchor, text),
                    }
                    Inserted::Taken
                }
                Input::Idle | Input::Ended { .. } => match replacement {
                    None => {
                        let selection = active.document.selection;
                        replaced(&mut active.document, selection, text);
                        Inserted::Typed
                    }
                    Some(range) => {
                        let target = clamp(&active.document, range);
                        session.events.push(ImeEvent::Replace {
                            range: target,
                            text: text.into(),
                        });
                        replaced(&mut active.document, target, text);
                        Inserted::Taken
                    }
                },
            }
        })
    }

    pub fn marked_range(&self) -> Option<Utf16Range> {
        self.active()?.marked().map(Marked::range)
    }

    pub fn selected_range(&self) -> Option<Utf16Range> {
        let active = self.active()?;
        Some(match active.marked() {
            Some(marked) => Utf16Range::new(
                marked.anchor + marked.selected.start,
                marked.anchor + marked.selected.end,
            ),
            None => active.document.selection,
        })
    }

    pub fn attributed_substring(&self, range: Utf16Range) -> Option<(String, Utf16Range)> {
        let (start, text) = self.active()?.excerpt();
        let end = start + utf16_len(&text);
        if range.end < start || range.start > end {
            return None;
        }
        let covered = Utf16Range::new(range.start.max(start), range.end.min(end));
        let local = Utf16Range::new(covered.start - start, covered.end - start);
        Some((slice(&text, local).to_owned(), covered))
    }

    pub fn first_rect(
        &self,
        range: Utf16Range,
        map: ScreenMap,
    ) -> Option<((f64, f64, f64, f64), Utf16Range)> {
        let active = self.active()?;
        let document = &active.document;
        let line = first_line(&document.geometry, active.document_range(range));
        let bounds = document
            .geometry
            .range_bounds(line)
            .map(|bounds| bounds.rect)
            .or_else(|| {
                document
                    .geometry
                    .range_bounds(Utf16Range::new(line.start, line.start))
                    .map(|bounds| bounds.rect)
            })
            .or_else(|| has_area(document.area).then_some(document.area))?;
        let actual = match line.end == active.document_range(range).end {
            true => range,
            false => Utf16Range::new(range.start, active.to_virtual(line.end)),
        };
        Some((map.to_screen(bounds), actual))
    }

    pub fn character_index(&self, screen: (f64, f64), map: ScreenMap) -> Option<u32> {
        let active = self.active()?;
        let hit = active.document.geometry.hit(map.to_view(screen))?;
        hit.inside.map(|offset| active.to_virtual(offset))
    }
}
