pub(crate) struct StageClock(
    Option<std::time::Instant>,
    std::cell::Cell<std::time::Instant>,
);

impl StageClock {
    pub(crate) fn from_env() -> Self {
        let now = std::time::Instant::now();
        let enabled = std::env::var_os("AQUA_PROFILE").is_some_and(|value| value == "1");
        Self(enabled.then_some(now), std::cell::Cell::new(now))
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
        eprintln!(
            "aqua-startup stage={stage} ms={:.1} private_mib={:.1}",
            now.duration_since(self.1.replace(now)).as_secs_f64() * 1000.0,
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
