use std::ffi::c_void;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> *mut c_void;
    fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *const c_void;
    static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        layout: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        key_translate_options: u32,
        dead_key_state: *mut u32,
        max_string_length: usize,
        actual_string_length: *mut usize,
        unicode_string: *mut u16,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(object: *const c_void);
}

const KEY_ACTION_DOWN: u16 = 0;
const NO_DEAD_KEYS: u32 = 1;
const SHIFT_STATE: u32 = 0x02;

pub(crate) fn ascii_character(key_code: u16, shift: bool) -> Option<char> {
    let source = unsafe { TISCopyCurrentASCIICapableKeyboardLayoutInputSource() };
    if source.is_null() {
        return None;
    }
    let data = unsafe { TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) };
    let character = match data.is_null() {
        true => None,
        false => {
            let layout = unsafe { CFDataGetBytePtr(data) }.cast::<c_void>();
            let mut dead = 0u32;
            let mut length = 0usize;
            let mut buffer = [0u16; 4];
            let status = unsafe {
                UCKeyTranslate(
                    layout,
                    key_code,
                    KEY_ACTION_DOWN,
                    if shift { SHIFT_STATE } else { 0 },
                    u32::from(LMGetKbdType()),
                    NO_DEAD_KEYS,
                    &mut dead,
                    buffer.len(),
                    &mut length,
                    buffer.as_mut_ptr(),
                )
            };
            (status == 0)
                .then(|| {
                    char::decode_utf16(buffer[..length.min(buffer.len())].iter().copied()).next()
                })
                .flatten()
                .and_then(Result::ok)
        }
    };
    unsafe { CFRelease(source) };
    character
}
