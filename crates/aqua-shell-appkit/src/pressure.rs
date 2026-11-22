use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use block2::RcBlock;
use dispatch2::{
    dispatch_source_memorypressure_flags_t, DispatchObject, DispatchQueue, DispatchRetained,
    DispatchSource, GlobalQueueIdentifier,
};

pub type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Normal,
    Warning,
    Critical,
}

impl Level {
    fn from_data(data: usize) -> Level {
        let flags = data as std::ffi::c_ulong;
        let critical = dispatch_source_memorypressure_flags_t::DISPATCH_MEMORYPRESSURE_CRITICAL.0;
        let warn = dispatch_source_memorypressure_flags_t::DISPATCH_MEMORYPRESSURE_WARN.0;
        match (flags & critical != 0, flags & warn != 0) {
            (true, _) => Level::Critical,
            (false, true) => Level::Warning,
            (false, false) => Level::Normal,
        }
    }

    fn data(self) -> usize {
        let flags = match self {
            Level::Normal => dispatch_source_memorypressure_flags_t::DISPATCH_MEMORYPRESSURE_NORMAL,
            Level::Warning => dispatch_source_memorypressure_flags_t::DISPATCH_MEMORYPRESSURE_WARN,
            Level::Critical => {
                dispatch_source_memorypressure_flags_t::DISPATCH_MEMORYPRESSURE_CRITICAL
            }
        };
        flags.0 as usize
    }

    fn code(self) -> u8 {
        match self {
            Level::Normal => 0,
            Level::Warning => 1,
            Level::Critical => 2,
        }
    }

    fn of_code(code: u8) -> Level {
        match code {
            0 => Level::Normal,
            1 => Level::Warning,
            2 => Level::Critical,
            other => unreachable!("memory pressure level code {other} was never stored"),
        }
    }

    pub fn elevated(self) -> bool {
        !matches!(self, Level::Normal)
    }
}

struct Shared {
    level: AtomicU8,
    elevated_since_taken: AtomicBool,
    wake: Option<Wake>,
}

impl Shared {
    fn record(&self, level: Level) {
        self.level.store(level.code(), Ordering::Release);
        if level.elevated() {
            self.elevated_since_taken.store(true, Ordering::Release);
        }
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

struct SourcePointer(NonNull<DispatchSource>);

pub struct PressureSource {
    source: DispatchRetained<DispatchSource>,
    shared: Arc<Shared>,
}

unsafe impl Send for PressureSource {}

pub struct Injector {
    source: DispatchRetained<DispatchSource>,
}

unsafe impl Send for Injector {}

impl Injector {
    pub fn inject(&self, level: Level) {
        self.source.merge_data(level.data());
    }
}

fn global_queue() -> DispatchRetained<DispatchQueue> {
    DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(
        dispatch2::DispatchQoS::Utility,
    ))
}

impl PressureSource {
    fn listen(source: DispatchRetained<DispatchSource>, wake: Option<Wake>) -> PressureSource {
        let shared = Arc::new(Shared {
            level: AtomicU8::new(Level::Normal.code()),
            elevated_since_taken: AtomicBool::new(false),
            wake,
        });
        let target = SourcePointer(NonNull::from(&*source));
        let recorder = shared.clone();
        let handler = RcBlock::new(move || {
            let source = unsafe { target.0.as_ref() };
            recorder.record(Level::from_data(source.data()));
        });
        unsafe {
            source.set_event_handler_with_block(RcBlock::as_ptr(&handler));
        }
        source.activate();
        PressureSource { source, shared }
    }

    pub fn system(wake: Option<Wake>) -> PressureSource {
        let mask = dispatch_source_memorypressure_flags_t::DISPATCH_MEMORYPRESSURE_NORMAL.0
            | dispatch_source_memorypressure_flags_t::DISPATCH_MEMORYPRESSURE_WARN.0
            | dispatch_source_memorypressure_flags_t::DISPATCH_MEMORYPRESSURE_CRITICAL.0;
        let source = unsafe {
            DispatchSource::new(
                &raw const dispatch2::_dispatch_source_type_memorypressure as *mut _,
                0,
                mask as usize,
                Some(&global_queue()),
            )
        };
        Self::listen(source, wake)
    }

    pub fn injectable(wake: Option<Wake>) -> (PressureSource, Injector) {
        let source = unsafe {
            DispatchSource::new(
                &raw const dispatch2::_dispatch_source_type_data_replace as *mut _,
                0,
                0,
                Some(&global_queue()),
            )
        };
        let injector = Injector {
            source: source.clone(),
        };
        (Self::listen(source, wake), injector)
    }

    pub fn level(&self) -> Level {
        Level::of_code(self.shared.level.load(Ordering::Acquire))
    }

    pub fn take_elevated(&self) -> bool {
        let seen = self
            .shared
            .elevated_since_taken
            .swap(false, Ordering::AcqRel);
        seen || self.level().elevated()
    }
}

impl Drop for PressureSource {
    fn drop(&mut self) {
        self.source.cancel();
    }
}

impl std::fmt::Debug for PressureSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PressureSource")
            .field("level", &self.level())
            .finish_non_exhaustive()
    }
}
