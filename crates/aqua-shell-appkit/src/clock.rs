use aqua_protocol::Instant;
use std::sync::OnceLock;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Timebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

struct Clock {
    timebase: Timebase,
    origin: u64,
}

static CLOCK: OnceLock<Clock> = OnceLock::new();

fn clock() -> &'static Clock {
    CLOCK.get_or_init(|| {
        let mut timebase = Timebase::default();
        let status = unsafe { mach_timebase_info(&mut timebase) };
        assert!(
            status == 0 && timebase.denom != 0,
            "invariant: mach_timebase_info answers on every Mac (status {status})"
        );
        let ticks = unsafe { mach_absolute_time() };
        Clock {
            timebase,
            origin: nanos_of(timebase, ticks),
        }
    })
}

fn nanos_of(timebase: Timebase, ticks: u64) -> u64 {
    let wide = u128::from(ticks) * u128::from(timebase.numer) / u128::from(timebase.denom);
    u64::try_from(wide).unwrap_or(u64::MAX)
}

pub fn now() -> Instant {
    let clock = clock();
    let ticks = unsafe { mach_absolute_time() };
    Instant::from_nanos(nanos_of(clock.timebase, ticks).saturating_sub(clock.origin))
}

pub fn from_media_time(seconds: f64) -> Instant {
    let nanos = (seconds.max(0.0) * 1e9).round() as u64;
    Instant::from_nanos(nanos.saturating_sub(clock().origin))
}
