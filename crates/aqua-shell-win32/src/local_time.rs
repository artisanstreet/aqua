use windows::core::PCWSTR;
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::{
    GetDateFormatEx, GetTimeFormatEx, DATE_SHORTDATE, TIME_NOSECONDS,
};

#[derive(Clone, Copy, Debug)]
pub struct LocalDateParts {
    pub year: u16,
    pub month: u16,
    pub weekday: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
}

fn system_time(at: LocalDateParts) -> SYSTEMTIME {
    SYSTEMTIME {
        wYear: at.year,
        wMonth: at.month,
        wDayOfWeek: at.weekday,
        wDay: at.day,
        wHour: at.hour,
        wMinute: at.minute,
        wSecond: 0,
        wMilliseconds: 0,
    }
}

fn utf16(text: &[u16], written: i32) -> Option<String> {
    let written = usize::try_from(written).ok()?.checked_sub(1)?;
    String::from_utf16(text.get(..written)?).ok()
}

pub fn clock(at: LocalDateParts) -> Option<String> {
    let time = system_time(at);
    let mut text = [0_u16; 64];
    let written = unsafe {
        GetTimeFormatEx(
            PCWSTR::null(),
            TIME_NOSECONDS,
            Some(&raw const time),
            PCWSTR::null(),
            Some(&mut text),
        )
    };
    utf16(&text, written)
}

pub fn short_date(at: LocalDateParts) -> Option<String> {
    let date = system_time(at);
    let mut text = [0_u16; 64];
    let written = unsafe {
        GetDateFormatEx(
            PCWSTR::null(),
            DATE_SHORTDATE,
            Some(&raw const date),
            PCWSTR::null(),
            Some(&mut text),
            PCWSTR::null(),
        )
    };
    utf16(&text, written)
}
