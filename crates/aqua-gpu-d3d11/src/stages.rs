pub(crate) struct StageClock(
    Option<std::time::Instant>,
    std::cell::Cell<std::time::Instant>,
);

impl StageClock {
    pub(crate) fn from_env() -> Self {
        let now = std::time::Instant::now();
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let enabled = *ENABLED
            .get_or_init(|| std::env::var_os("AQUA_PROFILE").is_some_and(|value| value == "1"));
        Self(enabled.then_some(now), std::cell::Cell::new(now))
    }

    pub(crate) fn note(&self, text: &str) {
        if self.0.is_some() {
            record(process_ms(), format!("aqua-startup pipelines {text}"));
        }
    }

    pub(crate) fn mark(&self, stage: &str) {
        if self.0.is_none() {
            return;
        }
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.1.get()).as_secs_f64() * 1000.0;
        let at = process_ms();
        let private = match std::env::var_os("AQUA_MEMORY_REPORT").is_some_and(|value| value == "1")
        {
            true => private_mib(),
            false => 0.0,
        };
        record(
            at,
            format!(
                "aqua-startup stage={stage} ms={elapsed:.1} private_mib={private:.1} process_ms={at:.1}"
            ),
        );
        self.1.set(std::time::Instant::now());
    }
}

static LINES: std::sync::Mutex<Vec<(f64, String)>> = std::sync::Mutex::new(Vec::new());

fn record(at: f64, line: String) {
    LINES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((at, line));
}

pub fn drain() -> Vec<(f64, String)> {
    std::mem::take(
        &mut *LINES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

fn private_mib() -> f64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    let read = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
    };
    match read {
        Ok(()) => counters.PrivateUsage as f64 / 1048576.0,
        Err(_) => 0.0,
    }
}

fn process_ms() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    let read = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
    let unix_ticks = ticks.saturating_sub(116_444_736_000_000_000);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0);
    match read {
        Ok(()) => now - unix_ticks as f64 / 10_000.0,
        Err(_) => 0.0,
    }
}
