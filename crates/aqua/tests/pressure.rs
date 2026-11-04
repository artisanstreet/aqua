use aqua::render::pressure::{simulate_low_memory, MemoryPressure};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static GLOBAL_SOURCES: Mutex<()> = Mutex::new(());

#[test]
fn a_trim_fires_once_per_rising_edge_and_polls_at_most_every_two_seconds() {
    let _serial = GLOBAL_SOURCES.lock().unwrap_or_else(|p| p.into_inner());
    let mut pressure = MemoryPressure::new();
    let start = Instant::now();
    let at = |seconds: u64| start + Duration::from_secs(seconds);
    simulate_low_memory(false);
    assert!(!pressure.rising_edge(at(0)));
    simulate_low_memory(true);
    assert!(
        !pressure.rising_edge(at(1)),
        "a poll inside the interval is skipped"
    );
    assert!(pressure.rising_edge(at(2)));
    assert!(
        !pressure.rising_edge(at(5)),
        "sustained pressure trims once"
    );
    simulate_low_memory(false);
    assert!(!pressure.rising_edge(at(8)));
    simulate_low_memory(true);
    assert!(pressure.rising_edge(at(11)));
    simulate_low_memory(false);
}

#[cfg(windows)]
#[test]
fn a_signalled_kernel_event_is_read_as_low_memory_through_the_wait_path() {
    let _serial = GLOBAL_SOURCES.lock().unwrap_or_else(|p| p.into_inner());
    use aqua::render::pressure::watch_notification;
    use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};
    let event = unsafe { CreateEventW(None, true, false, None) }.unwrap();
    simulate_low_memory(false);
    watch_notification(Some(event));
    let mut pressure = MemoryPressure::new();
    let start = Instant::now();
    let at = |seconds: u64| start + Duration::from_secs(seconds);
    assert!(!pressure.rising_edge(at(0)));
    unsafe { SetEvent(event) }.unwrap();
    assert!(pressure.rising_edge(at(2)));
    assert!(!pressure.rising_edge(at(4)));
    unsafe { ResetEvent(event) }.unwrap();
    assert!(!pressure.rising_edge(at(6)));
    unsafe { SetEvent(event) }.unwrap();
    assert!(pressure.rising_edge(at(8)));
    watch_notification(None);
}
