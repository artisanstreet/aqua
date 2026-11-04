#![cfg(windows)]

use aqua_shell_win32::local_time::{clock, short_date, LocalDateParts};

fn parts(hour: u16, minute: u16) -> LocalDateParts {
    LocalDateParts {
        year: 2026,
        month: 11,
        weekday: 3,
        day: 4,
        hour,
        minute,
    }
}

#[test]
fn clock_formats_in_the_user_locale_without_seconds() {
    let morning = clock(parts(9, 5)).expect("clock");
    let evening = clock(parts(21, 5)).expect("clock");
    assert!(morning.contains('5') && morning.contains('9'), "{morning}");
    assert_ne!(morning, evening);
    assert_eq!(morning.matches(':').count(), 1, "{morning}");
}

#[test]
fn short_date_contains_the_day_and_year() {
    let date = short_date(parts(0, 0)).expect("date");
    assert!(date.contains("26") && date.contains('4'), "{date}");
}

#[test]
fn invalid_calendar_parts_yield_none() {
    let bad = LocalDateParts {
        month: 13,
        ..parts(0, 0)
    };
    assert_eq!(short_date(bad), None);
    assert_eq!(
        clock(LocalDateParts {
            hour: 99,
            ..parts(0, 0)
        }),
        None
    );
}
