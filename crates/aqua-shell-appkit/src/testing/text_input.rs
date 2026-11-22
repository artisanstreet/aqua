use crate::window::Window;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSAttributedString, NSNotFound, NSPoint, NSRange, NSRect, NSString, NSUInteger,
};

pub type Range = (usize, usize);

fn view(window: Window) -> Retained<crate::view::AquaView> {
    crate::state::window(window.id(), |state| state.view.clone())
        .expect("text input calls target a live window")
}

fn ns(range: Option<Range>) -> NSRange {
    match range {
        Some((start, len)) => NSRange::new(start, len),
        None => NSRange::new(NSNotFound as NSUInteger, 0),
    }
}

fn found(range: NSRange) -> Option<Range> {
    (range.location != NSNotFound as NSUInteger).then_some((range.location, range.length))
}

pub fn set_marked(window: Window, text: &str, selected: Range, replacement: Option<Range>) {
    let view = view(window);
    let string = NSString::from_str(text);
    let string: &AnyObject = &string;
    unsafe {
        msg_send![&*view, setMarkedText: string, selectedRange: ns(Some(selected)), replacementRange: ns(replacement)]
    }
}

pub fn insert(window: Window, text: &str, replacement: Option<Range>) {
    let view = view(window);
    let string = NSString::from_str(text);
    let string: &AnyObject = &string;
    unsafe { msg_send![&*view, insertText: string, replacementRange: ns(replacement)] }
}

pub fn unmark(window: Window) {
    let view = view(window);
    unsafe { msg_send![&*view, unmarkText] }
}

pub fn selected_range(window: Window) -> Option<Range> {
    let view = view(window);
    found(unsafe { msg_send![&*view, selectedRange] })
}

pub fn marked_range(window: Window) -> Option<Range> {
    let view = view(window);
    found(unsafe { msg_send![&*view, markedRange] })
}

pub fn attributed_substring(window: Window, range: Range) -> Option<(String, Range)> {
    let view = view(window);
    let mut actual = ns(None);
    let text: Option<Retained<NSAttributedString>> = unsafe {
        msg_send![&*view, attributedSubstringForProposedRange: ns(Some(range)), actualRange: &mut actual]
    };
    text.map(|text| (text.string().to_string(), (actual.location, actual.length)))
}

pub fn first_rect(window: Window, range: Range) -> ((f64, f64, f64, f64), Range) {
    let view = view(window);
    let mut actual = ns(None);
    let rect: NSRect = unsafe {
        msg_send![&*view, firstRectForCharacterRange: ns(Some(range)), actualRange: &mut actual]
    };
    (
        (
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        ),
        (actual.location, actual.length),
    )
}

pub fn character_index(window: Window, screen: (f64, f64)) -> Option<usize> {
    let view = view(window);
    let index: NSUInteger =
        unsafe { msg_send![&*view, characterIndexForPoint: NSPoint::new(screen.0, screen.1)] };
    (index != NSNotFound as NSUInteger).then_some(index)
}
