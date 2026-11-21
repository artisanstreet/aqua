use crate::looping::emit;
use crate::state::SharedClient;
use crate::text_input::Utf16Range;
use crate::view::AquaView;
use aqua_core::InputEvent;
use aqua_shell::ShellEvent;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass};
use objc2_foundation::{
    NSAttributedString, NSNotFound, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSString,
    NSUInteger,
};

fn string_of(object: &AnyObject) -> String {
    match (
        object.downcast_ref::<NSAttributedString>(),
        object.downcast_ref::<NSString>(),
    ) {
        (Some(attributed), _) => attributed.string().to_string(),
        (None, Some(plain)) => plain.to_string(),
        (None, None) => {
            unreachable!("NSTextInputClient receives an NSString or NSAttributedString")
        }
    }
}

fn client(view: &AquaView) -> Option<SharedClient> {
    crate::state::window(view.id(), |window| window.text_client.clone()).flatten()
}

fn not_found() -> NSRange {
    NSRange::new(NSNotFound as NSUInteger, 0)
}

fn range_of(range: NSRange) -> Option<Utf16Range> {
    (range.location != NSNotFound as NSUInteger).then_some(Utf16Range {
        start: range.location,
        len: range.length,
    })
}

fn ns_range(range: Option<Utf16Range>) -> NSRange {
    range.map_or_else(not_found, |range| NSRange::new(range.start, range.len))
}

fn write_actual(actual: NSRangePointer, range: NSRange) {
    if !actual.is_null() {
        unsafe { actual.write(range) };
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub(crate) fn insert(view: &AquaView, string: &AnyObject, replacement: NSRange) {
    let text = string_of(string);
    match client(view) {
        Some(client) => {
            client
                .borrow_mut()
                .insert_text(&text, range_of(replacement));
            if let Some(pass) = view.ivars().pass.borrow_mut().as_mut() {
                pass.delivered = true;
            }
        }
        None => {
            view.ivars().marked.replace(None);
            let mut pass = view.ivars().pass.borrow_mut();
            match pass.as_mut() {
                Some(pass) => pass
                    .inserted
                    .get_or_insert_with(String::new)
                    .push_str(&text),
                None => {
                    let typed = aqua_shell::input::control_free(&text);
                    if !typed.is_empty() {
                        emit(
                            view.id(),
                            ShellEvent::Input(InputEvent::TextInput { text: typed }),
                        );
                    }
                }
            }
        }
    }
}

pub(crate) fn command(view: &AquaView) {
    if let Some(pass) = view.ivars().pass.borrow_mut().as_mut() {
        pass.command = true;
    }
}

pub(crate) fn set_marked(
    view: &AquaView,
    string: &AnyObject,
    selected: NSRange,
    replacement: NSRange,
) {
    let text = string_of(string);
    if let Some(pass) = view.ivars().pass.borrow_mut().as_mut() {
        pass.composed = true;
    }
    match client(view) {
        Some(client) => {
            let selected = range_of(selected).unwrap_or(Utf16Range {
                start: utf16_len(&text),
                len: 0,
            });
            client
                .borrow_mut()
                .set_marked_text(&text, selected, range_of(replacement));
        }
        None => {
            view.ivars()
                .marked
                .replace((!text.is_empty()).then_some(text));
        }
    }
}

pub(crate) fn unmark(view: &AquaView) {
    match client(view) {
        Some(client) => client.borrow_mut().unmark_text(),
        None => {
            view.ivars().marked.replace(None);
        }
    }
}

pub(crate) fn selected_range(view: &AquaView) -> NSRange {
    client(view).map_or_else(not_found, |client| {
        ns_range(client.borrow().selected_range())
    })
}

pub(crate) fn marked_range(view: &AquaView) -> NSRange {
    match client(view) {
        Some(client) => ns_range(client.borrow().marked_range()),
        None => view
            .ivars()
            .marked
            .borrow()
            .as_deref()
            .map_or_else(not_found, |marked| NSRange::new(0, utf16_len(marked))),
    }
}

pub(crate) fn has_marked(view: &AquaView) -> bool {
    match client(view) {
        Some(client) => client.borrow().marked_range().is_some(),
        None => view.ivars().marked.borrow().is_some(),
    }
}

pub(crate) fn attributed_substring(
    view: &AquaView,
    range: NSRange,
    actual: NSRangePointer,
) -> Option<Retained<NSAttributedString>> {
    let requested = range_of(range)?;
    let (text, covered) = client(view)?.borrow().attributed_substring(requested)?;
    write_actual(actual, ns_range(Some(covered)));
    Some(NSAttributedString::initWithString(
        NSAttributedString::alloc(),
        &NSString::from_str(&text),
    ))
}

fn caret_on_screen(view: &AquaView) -> NSRect {
    let local = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
    let in_window = view.convertRect_toView(local, None);
    view.window()
        .map_or(in_window, |window| window.convertRectToScreen(in_window))
}

pub(crate) fn first_rect(view: &AquaView, range: NSRange, actual: NSRangePointer) -> NSRect {
    let answer = range_of(range).and_then(|requested| {
        client(view).and_then(|client| client.borrow().first_rect(requested))
    });
    match answer {
        Some(((x, y, width, height), covered)) => {
            write_actual(actual, ns_range(Some(covered)));
            NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
        }
        None => {
            write_actual(actual, range);
            caret_on_screen(view)
        }
    }
}

pub(crate) fn character_index(view: &AquaView, point: NSPoint) -> NSUInteger {
    client(view)
        .and_then(|client| client.borrow().character_index((point.x, point.y)))
        .unwrap_or(NSNotFound as NSUInteger)
}
