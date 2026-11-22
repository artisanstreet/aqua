use objc2_foundation::NSString;
use std::ffi::c_void;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCreateInputSourceList(properties: *const c_void, include_all: bool) -> *const c_void;
    fn TISCopyCurrentKeyboardInputSource() -> *mut c_void;
    fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *const c_void;
    fn TISEnableInputSource(source: *const c_void) -> i32;
    fn TISSelectInputSource(source: *const c_void) -> i32;
    static kTISPropertyInputSourceID: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFRetain(object: *const c_void) -> *const c_void;
    fn CFRelease(object: *const c_void);
}

pub const KOTOERI_HIRAGANA: &str = "com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputSourceError {
    #[error("input source {id} is not installed")]
    Missing { id: String },
    #[error("input source {id}: {step} failed with OSStatus {status}")]
    Refused {
        id: String,
        step: &'static str,
        status: i32,
    },
}

fn source_id(source: *const c_void) -> Option<String> {
    let id = unsafe { TISGetInputSourceProperty(source.cast_mut(), kTISPropertyInputSourceID) };
    (!id.is_null()).then(|| unsafe { &*id.cast::<NSString>() }.to_string())
}

fn find(id: &str) -> Option<*const c_void> {
    let list = unsafe { TISCreateInputSourceList(std::ptr::null(), true) };
    if list.is_null() {
        return None;
    }
    let count = unsafe { CFArrayGetCount(list) };
    let found = (0..count)
        .map(|index| unsafe { CFArrayGetValueAtIndex(list, index) })
        .find(|source| source_id(*source).as_deref() == Some(id))
        .map(|source| unsafe { CFRetain(source) });
    unsafe { CFRelease(list) };
    found
}

pub fn installed(id: &str) -> bool {
    match find(id) {
        Some(source) => {
            unsafe { CFRelease(source) };
            true
        }
        None => false,
    }
}

pub fn current() -> Option<String> {
    let source = unsafe { TISCopyCurrentKeyboardInputSource() };
    if source.is_null() {
        return None;
    }
    let id = source_id(source);
    unsafe { CFRelease(source) };
    id
}

pub struct Selected {
    previous: *mut c_void,
}

impl Drop for Selected {
    fn drop(&mut self) {
        if !self.previous.is_null() {
            unsafe {
                TISSelectInputSource(self.previous);
                CFRelease(self.previous);
            }
        }
    }
}

pub fn select(id: &str) -> Result<Selected, InputSourceError> {
    let source = find(id).ok_or_else(|| InputSourceError::Missing { id: id.into() })?;
    let previous = Selected {
        previous: unsafe { TISCopyCurrentKeyboardInputSource() },
    };
    let enabled = unsafe { TISEnableInputSource(source) };
    let selected = match enabled {
        0 => unsafe { TISSelectInputSource(source) },
        status => status,
    };
    unsafe { CFRelease(source) };
    match (enabled, selected) {
        (0, 0) => Ok(previous),
        (0, status) => Err(InputSourceError::Refused {
            id: id.into(),
            step: "TISSelectInputSource",
            status,
        }),
        (status, _) => Err(InputSourceError::Refused {
            id: id.into(),
            step: "TISEnableInputSource",
            status,
        }),
    }
}
