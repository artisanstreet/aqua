use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

static SIMULATED: AtomicBool = AtomicBool::new(false);

pub fn simulate_low_memory(low: bool) {
    SIMULATED.store(low, Ordering::Release);
}

const QUIET_AFTER: Duration = Duration::from_secs(4);
const LOW_AVAILABLE_PERCENT: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strain {
    Calm,
    Stalled { last: Instant },
}

impl Strain {
    pub fn observe(self, now: Instant, stalled: bool) -> Strain {
        match (self, stalled) {
            (_, true) => Strain::Stalled { last: now },
            (Strain::Stalled { last }, false) if now.duration_since(last) < QUIET_AFTER => self,
            _ => Strain::Calm,
        }
    }

    pub fn low(self) -> bool {
        matches!(self, Strain::Stalled { .. })
    }
}

pub fn low_available(meminfo: &str) -> Option<bool> {
    let field = |name: &str| {
        meminfo.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            rest.split_whitespace().next()?.parse::<u64>().ok()
        })
    };
    let total = field("MemTotal").filter(|total| *total > 0)?;
    let available = field("MemAvailable")?;
    Some(available * 100 < total * LOW_AVAILABLE_PERCENT)
}

#[derive(Debug)]
pub struct MemoryPressure {
    last_poll: Option<Instant>,
    low: bool,
    #[cfg(windows)]
    handle: Option<windows::Win32::Foundation::HANDLE>,
    #[cfg(target_os = "linux")]
    source: linux::Source,
    #[cfg(target_os = "linux")]
    strain: Strain,
    #[cfg(target_os = "macos")]
    source: aqua_shell_appkit::pressure::PressureSource,
    #[cfg(target_os = "macos")]
    strain: Strain,
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::fd::{AsFd, BorrowedFd, OwnedFd, RawFd};
    use std::sync::atomic::{AtomicI32, Ordering};

    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    use rustix::fs::{open, Mode, OFlags};

    const PRESSURE: &str = "/proc/pressure/memory";
    const MEMINFO: &str = "/proc/meminfo";
    const TRIGGER: &[u8] = b"some 150000 2000000\0";

    #[derive(Debug)]
    pub(super) enum Source {
        Trigger(OwnedFd),
        Meminfo,
    }

    #[derive(Debug, thiserror::Error)]
    pub(super) enum TriggerError {
        #[error("opening {PRESSURE}: {0}")]
        Open(#[source] rustix::io::Errno),
        #[error("registering a stall trigger on {PRESSURE}: {0}")]
        Register(#[source] rustix::io::Errno),
    }

    static INJECTED: AtomicI32 = AtomicI32::new(-1);

    pub(super) fn inject(fd: Option<RawFd>) {
        INJECTED.store(fd.unwrap_or(-1), Ordering::Release);
    }

    fn trigger() -> Result<OwnedFd, TriggerError> {
        let fd = open(
            PRESSURE,
            OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(TriggerError::Open)?;
        rustix::io::write(&fd, TRIGGER).map_err(TriggerError::Register)?;
        Ok(fd)
    }

    pub(super) fn open_source() -> Source {
        match trigger() {
            Ok(fd) => Source::Trigger(fd),
            Err(error) => {
                eprintln!("aqua: memory pressure falls back to {MEMINFO}: {error}");
                Source::Meminfo
            }
        }
    }

    fn ready(fd: BorrowedFd<'_>, wanted: PollFlags) -> bool {
        let mut fds = [PollFd::new(&fd, wanted)];
        let polled = poll(&mut fds, Some(&Timespec::default())).unwrap_or(0) > 0;
        let revents = fds[0].revents();
        polled && revents.contains(wanted) && !revents.contains(PollFlags::ERR)
    }

    fn stall_reported(trigger: BorrowedFd<'_>) -> bool {
        ready(trigger, PollFlags::PRI)
    }

    fn injected_event(fd: BorrowedFd<'_>) -> bool {
        let fired = ready(fd, PollFlags::IN);
        if fired {
            let mut drained = [0u8; 8];
            let _ = rustix::io::read(fd, &mut drained);
        }
        fired
    }

    pub(super) fn stalled(source: &Source) -> bool {
        let injected = match INJECTED.load(Ordering::Acquire) {
            -1 => false,
            raw => injected_event(unsafe { BorrowedFd::borrow_raw(raw) }),
        };
        let own = match source {
            Source::Trigger(fd) => stall_reported(fd.as_fd()),
            Source::Meminfo => std::fs::read_to_string(MEMINFO)
                .ok()
                .and_then(|meminfo| super::low_available(&meminfo))
                .unwrap_or(false),
        };
        injected || own
    }
}

#[cfg(target_os = "linux")]
pub fn watch_notification(fd: Option<std::os::fd::RawFd>) {
    linux::inject(fd);
}

#[cfg(windows)]
fn open() -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::System::Memory::{
        CreateMemoryResourceNotification, LowMemoryResourceNotification,
    };
    unsafe { CreateMemoryResourceNotification(LowMemoryResourceNotification) }.ok()
}

#[cfg(windows)]
static INJECTED: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[cfg(windows)]
pub fn watch_notification(handle: Option<windows::Win32::Foundation::HANDLE>) {
    INJECTED.store(
        handle.map_or(0, |handle| handle.0 as isize),
        Ordering::Release,
    );
}

#[cfg(windows)]
fn signalled(handle: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::WaitForSingleObject;
    let state = unsafe { WaitForSingleObject(handle, 0) };
    state == WAIT_OBJECT_0
}

impl MemoryPressure {
    #[cfg(not(target_os = "macos"))]
    pub fn new() -> Self {
        Self {
            last_poll: None,
            low: false,
            #[cfg(windows)]
            handle: open(),
            #[cfg(target_os = "linux")]
            source: linux::open_source(),
            #[cfg(target_os = "linux")]
            strain: Strain::Calm,
        }
    }

    #[cfg(target_os = "macos")]
    pub fn new() -> Self {
        Self::on(aqua_shell_appkit::pressure::PressureSource::system(None))
    }

    #[cfg(target_os = "macos")]
    pub fn on(source: aqua_shell_appkit::pressure::PressureSource) -> Self {
        Self {
            last_poll: None,
            low: false,
            source,
            strain: Strain::Calm,
        }
    }

    #[cfg(target_os = "linux")]
    pub fn watches_the_kernel(&self) -> bool {
        matches!(self.source, linux::Source::Trigger(_))
    }

    #[cfg(windows)]
    fn sample(&mut self, _now: Instant) -> bool {
        let injected = match INJECTED.load(Ordering::Acquire) {
            0 => None,
            raw => Some(windows::Win32::Foundation::HANDLE(raw as *mut _)),
        };
        SIMULATED.load(Ordering::Acquire)
            || self.handle.is_some_and(signalled)
            || injected.is_some_and(signalled)
    }

    #[cfg(target_os = "linux")]
    fn sample(&mut self, now: Instant) -> bool {
        self.strain = self.strain.observe(now, linux::stalled(&self.source));
        SIMULATED.load(Ordering::Acquire) || self.strain.low()
    }

    #[cfg(target_os = "macos")]
    fn sample(&mut self, now: Instant) -> bool {
        self.strain = self.strain.observe(now, self.source.take_elevated());
        SIMULATED.load(Ordering::Acquire) || self.strain.low()
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    fn sample(&mut self, _now: Instant) -> bool {
        SIMULATED.load(Ordering::Acquire)
    }

    pub fn rising_edge(&mut self, now: Instant) -> bool {
        let due = self
            .last_poll
            .is_none_or(|last| now.duration_since(last) >= POLL_INTERVAL);
        if !due {
            return false;
        }
        self.last_poll = Some(now);
        let low = self.sample(now);
        let edge = low && !self.low;
        self.low = low;
        edge
    }
}

impl Default for MemoryPressure {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl Drop for MemoryPressure {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        }
    }
}
