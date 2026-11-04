use std::fmt;

pub const MIB: u64 = 1024 * 1024;

pub const RENDERER_OFFSCREEN: MemoryBudget = MemoryBudget {
    private_total: 96 * MIB,
    framework_heap: 4 * MIB,
    steady_growth: MIB,
    vram_total: 48 * MIB,
};

pub const RENDERER_WINDOWED: MemoryBudget = MemoryBudget {
    private_total: 140 * MIB,
    framework_heap: 4 * MIB,
    steady_growth: MIB,
    vram_total: 64 * MIB,
};

pub const HOST_WINDOWED: MemoryBudget = MemoryBudget {
    framework_heap: 12 * MIB,
    ..RENDERER_WINDOWED
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryBudget {
    pub private_total: u64,
    pub framework_heap: u64,
    pub steady_growth: u64,
    pub vram_total: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    pub private_total: u64,
    pub framework_heap: u64,
    pub steady_growth: u64,
    pub vram_total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Breach {
    PrivateTotal { measured: u64, limit: u64 },
    FrameworkHeap { measured: u64, limit: u64 },
    SteadyGrowth { measured: u64, limit: u64 },
    VramTotal { measured: u64, limit: u64 },
}

impl fmt::Display for Breach {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (name, measured, limit) = match *self {
            Breach::PrivateTotal { measured, limit } => ("private bytes", measured, limit),
            Breach::FrameworkHeap { measured, limit } => ("framework heap", measured, limit),
            Breach::SteadyGrowth { measured, limit } => ("steady-state growth", measured, limit),
            Breach::VramTotal { measured, limit } => ("video memory", measured, limit),
        };
        write!(
            f,
            "{name}: {:.2} MiB over the {:.2} MiB limit",
            measured as f64 / MIB as f64,
            limit as f64 / MIB as f64
        )
    }
}

impl MemoryBudget {
    pub fn check(&self, measured: &Measured) -> Vec<Breach> {
        [
            (measured.private_total > self.private_total).then_some(Breach::PrivateTotal {
                measured: measured.private_total,
                limit: self.private_total,
            }),
            (measured.framework_heap > self.framework_heap).then_some(Breach::FrameworkHeap {
                measured: measured.framework_heap,
                limit: self.framework_heap,
            }),
            (measured.steady_growth > self.steady_growth).then_some(Breach::SteadyGrowth {
                measured: measured.steady_growth,
                limit: self.steady_growth,
            }),
            (measured.vram_total > self.vram_total).then_some(Breach::VramTotal {
                measured: measured.vram_total,
                limit: self.vram_total,
            }),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}
