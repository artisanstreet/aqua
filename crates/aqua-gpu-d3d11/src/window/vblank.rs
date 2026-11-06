use std::time::{Duration, Instant};

use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput};
use windows::Win32::Graphics::Gdi::HMONITOR;

use super::pacing::VblankGuard;

const RETRY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct VblankClock {
    monitor: isize,
    factory: Option<IDXGIFactory1>,
    output: Option<IDXGIOutput>,
    retry_after: Option<Instant>,
    guard: VblankGuard,
}

impl VblankClock {
    pub fn wait(&mut self, monitor: isize, interval: Duration) -> bool {
        let now = Instant::now();
        let stale = self
            .factory
            .as_ref()
            .is_some_and(|factory| !unsafe { factory.IsCurrent() }.as_bool());
        let retry = self.output.is_none() && self.retry_after.is_none_or(|at| now >= at);
        if monitor != self.monitor || stale || retry {
            self.monitor = monitor;
            self.factory = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }.ok();
            self.output = self
                .factory
                .as_ref()
                .and_then(|factory| output_for(factory, monitor));
            self.retry_after = Some(now + RETRY);
        }
        let Some(output) = self.output.as_ref() else {
            return false;
        };
        let started = Instant::now();
        let waited = unsafe { output.WaitForVBlank() };
        if waited.is_ok() && self.guard.observe(started, Instant::now(), interval) {
            return true;
        }
        self.output = None;
        self.guard = VblankGuard::default();
        self.retry_after = Some(now + RETRY);
        false
    }
}

fn output_for(factory: &IDXGIFactory1, monitor: isize) -> Option<IDXGIOutput> {
    let monitor = HMONITOR(monitor as *mut _);
    (0..)
        .map_while(|adapter| unsafe { factory.EnumAdapters1(adapter) }.ok())
        .flat_map(|adapter| {
            (0..).map_while(move |index| unsafe { adapter.EnumOutputs(index) }.ok())
        })
        .find(|output| {
            unsafe { output.GetDesc() }
                .is_ok_and(|desc| desc.Monitor == monitor && desc.AttachedToDesktop.as_bool())
        })
}
