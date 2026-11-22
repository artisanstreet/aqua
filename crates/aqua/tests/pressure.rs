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

#[cfg(target_os = "linux")]
#[test]
fn a_stall_event_reads_as_low_memory_until_the_stall_has_been_quiet_for_two_windows() {
    let _serial = GLOBAL_SOURCES.lock().unwrap_or_else(|p| p.into_inner());
    use aqua::render::pressure::watch_notification;
    use rustix::event::{eventfd, EventfdFlags};
    use std::os::fd::AsRawFd;
    let stall = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK).unwrap();
    let fire = || {
        rustix::io::write(&stall, &1u64.to_ne_bytes()).unwrap();
    };
    simulate_low_memory(false);
    watch_notification(Some(stall.as_raw_fd()));
    let mut pressure = MemoryPressure::new();
    let start = Instant::now();
    let at = |seconds: u64| start + Duration::from_secs(seconds);
    assert!(!pressure.rising_edge(at(0)));
    fire();
    assert!(pressure.rising_edge(at(2)), "a stall event trims");
    assert!(
        !pressure.rising_edge(at(4)),
        "a quiet poll inside the hold keeps the pressure"
    );
    fire();
    assert!(
        !pressure.rising_edge(at(6)),
        "another stall during the episode trims nothing more"
    );
    assert!(!pressure.rising_edge(at(8)));
    assert!(
        !pressure.rising_edge(at(10)),
        "two quiet windows end the episode"
    );
    fire();
    assert!(pressure.rising_edge(at(12)), "the next episode trims again");
    watch_notification(None);
}

#[cfg(target_os = "macos")]
#[test]
fn an_injected_pressure_level_trims_once_and_ends_after_the_quiet_hold() {
    let _serial = GLOBAL_SOURCES.lock().unwrap_or_else(|p| p.into_inner());
    use aqua_shell_appkit::pressure::{Level, PressureSource};
    use std::sync::mpsc::sync_channel;
    use std::sync::Arc;
    let (sender, events) = sync_channel(8);
    let (source, injector) = PressureSource::injectable(Some(Arc::new(move || {
        let _delivered = sender.try_send(());
    })));
    let inject = |level| {
        injector.inject(level);
        events
            .recv_timeout(Duration::from_secs(5))
            .expect("the dispatch handler runs for the injected level");
    };
    simulate_low_memory(false);
    let mut pressure = MemoryPressure::on(source);
    let start = Instant::now();
    let at = |seconds: u64| start + Duration::from_secs(seconds);
    assert!(!pressure.rising_edge(at(0)));
    inject(Level::Warning);
    assert!(pressure.rising_edge(at(2)), "a warning trims");
    assert!(
        !pressure.rising_edge(at(4)),
        "the warning holds without trimming again"
    );
    inject(Level::Normal);
    assert!(
        !pressure.rising_edge(at(6)),
        "inside the quiet hold nothing trims"
    );
    assert!(!pressure.rising_edge(at(8)));
    assert!(!pressure.rising_edge(at(10)));
    inject(Level::Critical);
    assert!(pressure.rising_edge(at(12)), "the next episode trims again");
}

#[test]
fn low_available_memory_is_read_from_meminfo_below_five_percent() {
    use aqua::render::pressure::low_available;
    let meminfo = |available: u64| {
        format!("MemTotal:       16000000 kB\nMemFree:         100000 kB\nMemAvailable:   {available} kB\n")
    };
    assert_eq!(low_available(&meminfo(790_000)), Some(true));
    assert_eq!(low_available(&meminfo(800_000)), Some(false));
    assert_eq!(low_available("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
    assert_eq!(low_available("MemTotal: 100 kB\n"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn this_kernel_accepts_an_unprivileged_memory_stall_trigger() {
    if !std::path::Path::new("/proc/pressure/memory").exists() {
        eprintln!("SKIP-NO-PSI: /proc/pressure/memory is missing (kernel without CONFIG_PSI)");
        return;
    }
    let release = rustix::system::uname()
        .release()
        .to_string_lossy()
        .into_owned();
    let version: Vec<u32> = release
        .split(|c: char| !c.is_ascii_digit())
        .take(2)
        .filter_map(|part| part.parse().ok())
        .collect();
    if version.as_slice() < [6, 5].as_slice() {
        eprintln!("SKIP-OLD-KERNEL: {release} predates unprivileged PSI triggers (Linux 6.5)");
        return;
    }
    assert!(
        MemoryPressure::new().watches_the_kernel(),
        "a 150 ms stall in a 2 s window is a trigger an unprivileged process may register"
    );
}
