use aqua_core::{ImeEvent as Composition, Rect};
use aqua_protocol::{ImeCommand, ImeDocument, ImeEvent, Utf16Range};

pub const SURROUNDING_LIMIT: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excerpt {
    pub text: String,
    pub base: u32,
    pub cursor: u32,
    pub anchor: u32,
}

fn utf16_len(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX)
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

fn floor_boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

fn ceil_boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte += 1;
    }
    byte
}

fn window_span(len: usize, low: usize, high: usize, cursor: usize) -> (usize, usize) {
    let (low, high) = match high - low >= SURROUNDING_LIMIT {
        true => (cursor, cursor),
        false => (low, high),
    };
    let spare = SURROUNDING_LIMIT - (high - low);
    let start = low.saturating_sub(spare / 2);
    let end = (start + SURROUNDING_LIMIT).min(len);
    (end.saturating_sub(SURROUNDING_LIMIT).min(start), end)
}

impl Excerpt {
    pub fn of(document: &ImeDocument) -> Excerpt {
        let text = document.text.as_str();
        let window_end = document.window_start + utf16_len(text);
        let local = |offset: u32| {
            byte_at_utf16(
                text,
                offset
                    .clamp(document.window_start, window_end)
                    .saturating_sub(document.window_start),
            )
        };
        let (start, end) = (
            local(document.selection.start),
            local(document.selection.end),
        );
        let (cursor, anchor) = match document.selection_reversed {
            true => (start, end),
            false => (end, start),
        };
        let (low, high) = match text.len() <= SURROUNDING_LIMIT {
            true => (0, text.len()),
            false => {
                let (low, high) =
                    window_span(text.len(), cursor.min(anchor), cursor.max(anchor), cursor);
                (ceil_boundary(text, low), floor_boundary(text, high))
            }
        };
        let clamp = |byte: usize| byte.clamp(low, high) - low;
        Excerpt {
            text: text[low..high].to_owned(),
            base: document.window_start + utf16_len(&text[..low]),
            cursor: clamp(cursor) as u32,
            anchor: clamp(anchor) as u32,
        }
    }

    pub fn utf16(&self, byte: i64) -> u32 {
        let byte = usize::try_from(byte.max(0)).unwrap_or(usize::MAX);
        self.base + utf16_len(&self.text[..floor_boundary(&self.text, byte)])
    }

    pub fn deletion(&self, before: u32, after: u32) -> Utf16Range {
        let low = i64::from(self.cursor.min(self.anchor));
        let high = i64::from(self.cursor.max(self.anchor));
        Utf16Range::new(
            self.utf16(low - i64::from(before)),
            self.utf16(high + i64::from(after)),
        )
    }

    fn selection(&self) -> Utf16Range {
        Utf16Range::new(
            self.utf16(i64::from(self.cursor.min(self.anchor))),
            self.utf16(i64::from(self.cursor.max(self.anchor))),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaretBox {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub fn caret_box(document: &ImeDocument) -> Option<CaretBox> {
    let rect: Rect = document
        .geometry
        .range_bounds(document.selection)
        .map(|bounds| bounds.rect)
        .unwrap_or(document.area);
    let (x, y) = (rect.x.floor(), rect.y.floor());
    let finite = [rect.x, rect.y, rect.w, rect.h]
        .iter()
        .all(|v| v.is_finite());
    finite.then(|| CaretBox {
        x: x as i32,
        y: y as i32,
        width: ((rect.x + rect.w).ceil() - x).max(1.0) as i32,
        height: ((rect.y + rect.h).ceil() - y).max(1.0) as i32,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub surrounding: Excerpt,
    pub caret: Option<CaretBox>,
    pub changed_by_input_method: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    pub preedit: Option<(String, Option<(usize, usize)>)>,
    pub commit: Option<String>,
    pub delete: Option<(u32, u32)>,
    pub serial: u32,
}

pub trait Link {
    fn enable(&mut self, state: &Outgoing) -> Option<u32>;
    fn update(&mut self, state: &Outgoing) -> Option<u32>;
    fn disable(&mut self) -> Option<u32>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Reconversion,
}

#[derive(Debug, Clone, PartialEq)]
enum Guard {
    Open,
    Echo(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Active {
    document: ImeDocument,
    activation: u32,
    composing: Option<String>,
    guard: Guard,
    sent: Outgoing,
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Off,
    Waiting(ImeDocument),
    Active(Active),
}

#[derive(Debug)]
pub struct Session {
    phase: Phase,
    focused: bool,
    changed_by_input_method: bool,
    events: Vec<ImeEvent>,
}

fn newer_or_same(serial: u32, activation: u32) -> bool {
    (serial.wrapping_sub(activation) as i32) >= 0
}

fn replaced(document: &mut ImeDocument, range: Utf16Range, text: &str) {
    let window_end = document.window_start + utf16_len(&document.text);
    if range.start >= document.window_start && range.end <= window_end {
        let start = byte_at_utf16(&document.text, range.start - document.window_start);
        let end = byte_at_utf16(&document.text, range.end - document.window_start);
        document.text.replace_range(start..end, text);
    }
    let inserted = utf16_len(text);
    document.total_len = document.total_len.saturating_sub(range.len()) + inserted;
    let caret = range.start + inserted;
    document.selection = Utf16Range::new(caret, caret);
    document.selection_reversed = false;
}

fn inserted_at_selection(document: &mut ImeDocument, text: &str) {
    let selection = document.selection;
    replaced(document, selection, text);
}

impl Session {
    pub fn new(focused: bool) -> Session {
        Session {
            phase: Phase::Off,
            focused,
            changed_by_input_method: false,
            events: Vec::new(),
        }
    }

    pub fn is_composing(&self) -> bool {
        matches!(&self.phase, Phase::Active(active) if active.composing.is_some())
    }

    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Active(_))
    }

    pub fn drain(&mut self, out: &mut Vec<ImeEvent>) {
        out.append(&mut self.events);
    }

    fn outgoing(&self, document: &ImeDocument) -> Outgoing {
        Outgoing {
            surrounding: Excerpt::of(document),
            caret: caret_box(document),
            changed_by_input_method: self.changed_by_input_method,
        }
    }

    fn activate(&mut self, document: ImeDocument, guard: Guard, link: &mut impl Link) {
        let state = self.outgoing(&document);
        self.changed_by_input_method = false;
        self.phase = match link.enable(&state) {
            Some(activation) => Phase::Active(Active {
                document,
                activation,
                composing: None,
                guard,
                sent: state,
            }),
            None => Phase::Waiting(document),
        };
    }

    fn reset(&mut self, document: ImeDocument, guard: Guard, link: &mut impl Link) {
        link.disable();
        self.activate(document, guard, link);
    }

    fn end_composition(&mut self, outcome: Composition, link: &mut impl Link) {
        let Phase::Active(active) = std::mem::replace(&mut self.phase, Phase::Off) else {
            unreachable!("only an active session ends a composition");
        };
        let Some(preedit) = active.composing else {
            self.phase = Phase::Active(active);
            return;
        };
        let mut document = active.document;
        if let Composition::Commit { text } = &outcome {
            inserted_at_selection(&mut document, text);
        }
        self.events.push(ImeEvent::Composition(outcome));
        self.reset(document, Guard::Echo(preedit), link);
    }

    pub fn command(&mut self, command: ImeCommand, link: &mut impl Link) -> Result<(), Refusal> {
        match (command, &mut self.phase) {
            (ImeCommand::Reconvert, _) => return Err(Refusal::Reconversion),
            (ImeCommand::Enable(document), Phase::Active(active)) => {
                if active.composing.take().is_some() {
                    self.events.push(ImeEvent::Composition(Composition::Cancel));
                }
                self.reset(document, Guard::Open, link);
            }
            (ImeCommand::Enable(document), _) => match self.focused {
                true => self.activate(document, Guard::Open, link),
                false => self.phase = Phase::Waiting(document),
            },
            (ImeCommand::Update(document), Phase::Waiting(waiting)) => *waiting = document,
            (ImeCommand::Update(document), Phase::Active(_)) => {
                let state = self.outgoing(&document);
                let Phase::Active(active) = &mut self.phase else {
                    unreachable!("matched an active session above");
                };
                active.document = document;
                let unchanged = active.sent.surrounding == state.surrounding
                    && active.sent.caret == state.caret;
                if !unchanged {
                    link.update(&state);
                    active.sent = state;
                    self.changed_by_input_method = false;
                }
            }
            (ImeCommand::Update(_), Phase::Off) => {}
            (ImeCommand::Disable, Phase::Active(active)) => {
                if let Some(preedit) = active.composing.take() {
                    self.events
                        .push(ImeEvent::Composition(Composition::Commit { text: preedit }));
                }
                link.disable();
                self.phase = Phase::Off;
            }
            (ImeCommand::Disable, _) => self.phase = Phase::Off,
            (ImeCommand::CommitComposition, Phase::Active(active)) => {
                let text = active.composing.clone().unwrap_or_default();
                self.end_composition(Composition::Commit { text }, link);
            }
            (ImeCommand::CancelComposition, Phase::Active(_)) => {
                self.end_composition(Composition::Cancel, link);
            }
            (ImeCommand::CommitComposition | ImeCommand::CancelComposition, _) => {}
        }
        Ok(())
    }

    pub fn entered(&mut self, link: &mut impl Link) {
        self.focused = true;
        match std::mem::replace(&mut self.phase, Phase::Off) {
            Phase::Off => {}
            Phase::Waiting(document) => self.activate(document, Guard::Open, link),
            Phase::Active(active) => self.activate(active.document, Guard::Open, link),
        }
    }

    pub fn left(&mut self) {
        self.focused = false;
        self.phase = match std::mem::replace(&mut self.phase, Phase::Off) {
            Phase::Active(mut active) => {
                if let Some(preedit) = active.composing.take() {
                    inserted_at_selection(&mut active.document, &preedit);
                    self.events
                        .push(ImeEvent::Composition(Composition::Commit { text: preedit }));
                }
                Phase::Waiting(active.document)
            }
            other => other,
        };
    }

    pub fn done(&mut self, mut batch: Batch) {
        let Phase::Active(active) = &mut self.phase else {
            return;
        };
        if !newer_or_same(batch.serial, active.activation) {
            return;
        }
        if let Guard::Echo(echo) = std::mem::replace(&mut active.guard, Guard::Open)
            && batch.preedit.is_none()
            && batch.commit.as_deref() == Some(echo.as_str())
        {
            batch.commit = None;
        }
        let excerpt = Excerpt::of(&active.document);
        let target = match batch.delete {
            Some((before, after)) => excerpt.deletion(before, after),
            None => excerpt.selection(),
        };
        let edit = match (batch.commit, batch.delete) {
            (Some(text), _) => Some(text),
            (None, Some(_)) => Some(String::new()),
            (None, None) => None,
        };
        let mut events = Vec::new();
        if let Some(text) = edit {
            match (active.composing.take(), batch.delete) {
                (Some(_), None) => {
                    events.push(ImeEvent::Composition(Composition::Commit {
                        text: text.clone(),
                    }));
                }
                (Some(_), Some(_)) => {
                    events.push(ImeEvent::Composition(Composition::Cancel));
                    events.push(ImeEvent::Replace {
                        range: target,
                        text: text.clone(),
                    });
                }
                (None, _) => events.push(ImeEvent::Replace {
                    range: target,
                    text: text.clone(),
                }),
            }
            replaced(&mut active.document, target, &text);
            self.changed_by_input_method = true;
        }
        match (batch.preedit, active.composing.is_some()) {
            (Some((marked, cursor)), composing) => {
                if !composing {
                    events.push(ImeEvent::Composition(Composition::Start));
                }
                events.push(ImeEvent::Composition(Composition::Update {
                    marked: marked.clone(),
                    cursor,
                }));
                active.composing = Some(marked);
            }
            (None, true) => {
                active.composing = None;
                events.push(ImeEvent::Composition(Composition::Cancel));
            }
            (None, false) => {}
        }
        self.events.extend(events);
    }
}
