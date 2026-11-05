pub(crate) struct StageClock(
    Option<std::time::Instant>,
    std::cell::Cell<std::time::Instant>,
    std::cell::Cell<f64>,
);

impl StageClock {
    pub(crate) fn from_env() -> Self {
        let now = std::time::Instant::now();
        let enabled = std::env::var_os("AQUA_PROFILE").is_some_and(|value| value == "1");
        Self(
            enabled.then_some(now),
            std::cell::Cell::new(now),
            std::cell::Cell::new(thread_cpu_ms()),
        )
    }

    pub(crate) fn note(&self, text: &str) {
        if self.0.is_some() {
            eprintln!("aqua-startup pipelines {text}");
        }
    }

    pub(crate) fn mark(&self, stage: &str) {
        if self.0.is_none() {
            return;
        }
        let now = std::time::Instant::now();
        let cpu = thread_cpu_ms();
        eprintln!(
            "aqua-startup stage={stage} ms={:.1} cpu_ms={:.1} private_mib={:.1}",
            now.duration_since(self.1.replace(now)).as_secs_f64() * 1000.0,
            cpu - self.2.replace(cpu),
            private_mib()
        );
    }
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

fn thread_cpu_ms() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    let read = unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    let ticks =
        |time: FILETIME| ((time.dwHighDateTime as u64) << 32 | time.dwLowDateTime as u64) as f64;
    match read {
        Ok(()) => (ticks(kernel) + ticks(user)) / 10_000.0,
        Err(_) => 0.0,
    }
}
