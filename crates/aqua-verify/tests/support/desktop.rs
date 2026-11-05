#![allow(dead_code, unsafe_code)]

use std::ffi::c_void;

const WTS_CURRENT_SESSION: u32 = u32::MAX;
const WTS_SESSION_INFO_EX: i32 = 25;
const SESSION_FLAGS_OFFSET: usize = 16;
const SESSION_UNLOCKED: i32 = 1;

#[link(name = "wtsapi32")]
unsafe extern "system" {
    fn WTSQuerySessionInformationW(
        server: *mut c_void,
        session: u32,
        class: i32,
        buffer: *mut *mut u8,
        length: *mut u32,
    ) -> i32;
    fn WTSFreeMemory(memory: *mut c_void);
}

pub fn unlocked() -> bool {
    let mut buffer: *mut u8 = std::ptr::null_mut();
    let mut length = 0u32;
    let queried = unsafe {
        WTSQuerySessionInformationW(
            std::ptr::null_mut(),
            WTS_CURRENT_SESSION,
            WTS_SESSION_INFO_EX,
            &mut buffer,
            &mut length,
        )
    } != 0;
    match queried && !buffer.is_null() && length as usize >= SESSION_FLAGS_OFFSET + 4 {
        true => {
            let flags = unsafe {
                buffer
                    .add(SESSION_FLAGS_OFFSET)
                    .cast::<i32>()
                    .read_unaligned()
            };
            unsafe { WTSFreeMemory(buffer.cast()) };
            flags == SESSION_UNLOCKED
        }
        false => true,
    }
}

pub fn require(test: &str) -> bool {
    match unlocked() {
        true => true,
        false => {
            eprintln!(
                "SKIP-SESSION-LOCKED {test}: the user session is locked, so desktop capture reads black"
            );
            false
        }
    }
}
