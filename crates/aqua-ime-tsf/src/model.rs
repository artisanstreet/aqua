use aqua_core::ImeEvent as Composition;
use aqua_protocol::{ImeDocument, ImeEvent, Utf16Range};
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    InvalidPosition,
    ReadOnlyWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextChange {
    pub start: u32,
    pub old_end: u32,
    pub new_end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Notify {
    pub text: Option<TextChange>,
    pub selection: bool,
    pub layout: bool,
}

impl Notify {
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && !self.selection && !self.layout
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start: u32,
    pub end: u32,
    pub end_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug)]
pub struct Model {
    window_start: u32,
    total_len: u32,
    view: Vec<u16>,
    selection: Selection,
    composition: Option<Range<u32>>,
    area: Area,
    events: Vec<ImeEvent>,
    last_marked: Option<(Vec<u16>, Selection)>,
    app_selection: Range<u32>,
    last_edit: Option<LastEdit>,
}

#[derive(Debug, Clone)]
struct LastEdit {
    old: Range<u32>,
    new: Range<u32>,
    event_index: usize,
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub fn utf16_to_byte_offset(units: &[u16], index: usize) -> usize {
    let index = index.min(units.len());
    String::from_utf16_lossy(&units[..index]).len()
}

impl Model {
    pub fn new(document: &ImeDocument) -> Self {
        let mut model = Self {
            window_start: 0,
            total_len: 0,
            view: Vec::new(),
            selection: Selection {
                start: 0,
                end: 0,
                end_active: true,
            },
            composition: None,
            area: Area {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            events: Vec::new(),
            last_marked: None,
            app_selection: 0..0,
            last_edit: None,
        };
        model.load(document);
        model
    }

    fn load(&mut self, document: &ImeDocument) {
        self.window_start = document.window_start;
        self.total_len = document.total_len;
        self.view = document.text.encode_utf16().collect();
        let end = self.window_start + to_u32(self.view.len());
        self.total_len = self.total_len.max(end);
        self.selection = Selection {
            start: document.selection.start.min(self.total_len),
            end: document.selection.end.min(self.total_len),
            end_active: !document.selection_reversed,
        };
        self.area = area_of(document);
        self.app_selection = self.selection.start..self.selection.end;
        self.last_edit = None;
    }

    pub fn area(&self) -> Area {
        self.area
    }

    pub fn total_len(&self) -> u32 {
        self.total_len
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn composition(&self) -> Option<Range<u32>> {
        self.composition.clone()
    }

    pub fn drain(&mut self, out: &mut Vec<ImeEvent>) {
        out.append(&mut self.events);
    }

    fn window_end(&self) -> u32 {
        self.window_start + to_u32(self.view.len())
    }

    fn local(&self, start: u32, end: u32) -> Result<Range<usize>, EditError> {
        if start > end {
            return Err(EditError::InvalidPosition);
        }
        if start < self.window_start || end > self.window_end() {
            return Err(EditError::ReadOnlyWindow);
        }
        Ok((start - self.window_start) as usize..(end - self.window_start) as usize)
    }

    pub fn get_text(&self, start: u32, end: Option<u32>) -> Result<(&[u16], u32), EditError> {
        let end = end.unwrap_or(self.window_end()).min(self.window_end());
        if start > self.total_len {
            return Err(EditError::InvalidPosition);
        }
        let start = start.max(self.window_start).min(end);
        let range = self.local(start, end)?;
        Ok((&self.view[range], start))
    }

    pub fn set_selection(&mut self, selection: Selection) -> Result<(), EditError> {
        if selection.start > selection.end || selection.end > self.total_len {
            return Err(EditError::InvalidPosition);
        }
        if selection == self.selection {
            return Ok(());
        }
        self.selection = selection;
        self.after_selection_change();
        Ok(())
    }

    fn after_selection_change(&mut self) {
        match self.composition.clone() {
            Some(_) => self.publish_marked(),
            None => {
                self.app_selection = self.selection.start..self.selection.end;
                self.events.push(ImeEvent::Select {
                    range: Utf16Range::new(self.selection.start, self.selection.end),
                    reversed: !self.selection.end_active,
                });
            }
        }
    }

    pub fn replace(&mut self, start: u32, end: u32, text: &[u16]) -> Result<TextChange, EditError> {
        self.replace_with_caret(start, end, text, None)
    }

    fn replace_with_caret(
        &mut self,
        start: u32,
        end: u32,
        text: &[u16],
        caret: Option<u32>,
    ) -> Result<TextChange, EditError> {
        let local = self.local(start, end)?;
        let inserted = to_u32(text.len());
        let removed = end - start;
        let at_selection = start == self.selection.start && end == self.selection.end;
        self.view.splice(local, text.iter().copied());
        self.total_len = self.total_len - removed + inserted;
        let change = TextChange {
            start,
            old_end: end,
            new_end: start + inserted,
        };
        let contained = self.selection.start >= start && self.selection.end <= end;
        self.selection = match (caret, contained) {
            (Some(at), _) => Selection {
                start: at,
                end: at,
                end_active: true,
            },
            (None, true) => Selection {
                start: start + inserted,
                end: start + inserted,
                end_active: true,
            },
            (None, false) => Selection {
                start: shift(self.selection.start, start, end, inserted),
                end: shift(self.selection.end, start, end, inserted),
                end_active: self.selection.end_active,
            },
        };
        match self.composition.clone() {
            Some(composition) if touches(&composition, start, end) => {
                let new_start = composition.start.min(start);
                let new_end = composition.end.max(end);
                self.composition = Some(new_start..new_end - removed + inserted);
                self.publish_marked();
            }
            Some(composition) => {
                let moved = match end <= composition.start {
                    true => {
                        (composition.start - removed + inserted)
                            ..(composition.end - removed + inserted)
                    }
                    false => composition,
                };
                self.composition = Some(moved);
                self.events.push(ImeEvent::Replace {
                    range: Utf16Range::new(start, end),
                    text: String::from_utf16_lossy(text),
                });
            }
            None => self.publish_edit(start, end, text, at_selection),
        }
        Ok(change)
    }

    fn publish_edit(&mut self, start: u32, end: u32, text: &[u16], at_selection: bool) {
        let string = String::from_utf16_lossy(text);
        let event = match (at_selection, string.is_empty()) {
            (true, false) => ImeEvent::Composition(Composition::Commit { text: string }),
            _ => ImeEvent::Replace {
                range: Utf16Range::new(start, end),
                text: string,
            },
        };
        self.events.push(event);
        self.last_edit = Some(LastEdit {
            old: start..end,
            new: start..start + to_u32(text.len()),
            event_index: self.events.len() - 1,
        });
    }

    pub fn insert_at_selection(
        &mut self,
        text: &[u16],
    ) -> Result<(u32, u32, TextChange), EditError> {
        let Selection { start, end, .. } = self.selection;
        let caret = start + to_u32(text.len());
        let change = self.replace_with_caret(start, end, text, Some(caret))?;
        Ok((change.start, change.new_end, change))
    }

    pub fn start_composition(&mut self, range: Option<Range<u32>>) {
        if self.composition.is_some() {
            return;
        }
        let range = range.unwrap_or(self.selection.start..self.selection.start);
        let retracted = match self.last_edit.take() {
            Some(edit) if edit.new == range && edit.event_index + 1 == self.events.len() => {
                self.events.pop();
                Some(edit.old)
            }
            _ => None,
        };
        let anchor = retracted.clone().unwrap_or_else(|| range.clone());
        let collapsed_at_caret = anchor.is_empty() && anchor.start == self.app_selection.start;
        if anchor != self.app_selection && !collapsed_at_caret {
            self.events.push(ImeEvent::Select {
                range: Utf16Range::new(anchor.start, anchor.end),
                reversed: false,
            });
            self.app_selection = anchor;
        }
        self.composition = Some(range);
        self.last_marked = None;
        self.events.push(ImeEvent::Composition(Composition::Start));
        if retracted.is_some() {
            self.publish_marked();
        }
    }

    pub fn update_composition(&mut self, range: Range<u32>) {
        if self.composition.is_none() {
            return;
        }
        let end = range.end.min(self.window_end());
        let start = range.start.max(self.window_start).min(end);
        self.composition = Some(start..end);
        self.publish_marked();
    }

    fn marked_units(&self) -> Vec<u16> {
        match &self.composition {
            Some(range) => self
                .local(range.start, range.end)
                .map(|local| self.view[local].to_vec())
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    fn publish_marked(&mut self) {
        let Some(range) = self.composition.clone() else {
            return;
        };
        let units = self.marked_units();
        let snapshot = (units.clone(), self.selection);
        if self.last_marked.as_ref() == Some(&snapshot) {
            return;
        }
        self.last_marked = Some(snapshot);
        let cursor = {
            let relative =
                |at: u32| utf16_to_byte_offset(&units, at.saturating_sub(range.start) as usize);
            Some((relative(self.selection.start), relative(self.selection.end)))
        };
        self.events.push(ImeEvent::Composition(Composition::Update {
            marked: String::from_utf16_lossy(&units),
            cursor,
        }));
    }

    pub fn end_composition(&mut self) {
        let units = self.marked_units();
        if self.composition.take().is_none() {
            return;
        }
        self.last_marked = None;
        let event = match units.is_empty() {
            true => Composition::Cancel,
            false => Composition::Commit {
                text: String::from_utf16_lossy(&units),
            },
        };
        self.events.push(ImeEvent::Composition(event));
    }

    pub fn discard_composition(&mut self) -> Option<TextChange> {
        let range = self.composition.take()?;
        self.last_marked = None;
        let change = match self.local(range.start, range.end) {
            Ok(local) if !local.is_empty() => {
                self.view.drain(local);
                let removed = range.end - range.start;
                self.total_len -= removed;
                self.selection = Selection {
                    start: shift(self.selection.start, range.start, range.end, 0),
                    end: shift(self.selection.end, range.start, range.end, 0),
                    end_active: self.selection.end_active,
                };
                Some(TextChange {
                    start: range.start,
                    old_end: range.end,
                    new_end: range.start,
                })
            }
            _ => None,
        };
        self.events.push(ImeEvent::Composition(Composition::Cancel));
        change
    }

    pub fn apply_document(&mut self, document: &ImeDocument) -> Notify {
        let area = area_of(document);
        let layout = area != self.area;
        if self.composition.is_some() {
            self.area = area;
            return Notify {
                text: None,
                selection: false,
                layout,
            };
        }
        let old_units = std::mem::take(&mut self.view);
        let old_start = self.window_start;
        let old_selection = self.selection;
        self.load(document);
        let text = diff(old_start, &old_units, self.window_start, &self.view);
        Notify {
            text,
            selection: old_selection != self.selection,
            layout,
        }
    }
}

fn touches(composition: &Range<u32>, start: u32, end: u32) -> bool {
    start <= composition.end && end >= composition.start
}

fn shift(position: u32, start: u32, end: u32, inserted: u32) -> u32 {
    match position {
        p if p <= start => p,
        p if p >= end => p - (end - start) + inserted,
        _ => start + inserted,
    }
}

fn area_of(document: &ImeDocument) -> Area {
    Area {
        x: document.area.x,
        y: document.area.y,
        w: document.area.w,
        h: document.area.h,
    }
}

fn diff(old_start: u32, old: &[u16], new_start: u32, new: &[u16]) -> Option<TextChange> {
    if old_start != new_start {
        return Some(TextChange {
            start: old_start.min(new_start),
            old_end: old_start + to_u32(old.len()),
            new_end: new_start + to_u32(new.len()),
        });
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    Some(TextChange {
        start: old_start + to_u32(prefix),
        old_end: old_start + to_u32(old.len() - suffix),
        new_end: old_start + to_u32(new.len() - suffix),
    })
}
