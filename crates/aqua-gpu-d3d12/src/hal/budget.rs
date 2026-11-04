#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryBudget {
    pub budget: u64,
    pub usage: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    None,
    Soft,
    Hard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Visible,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Trim {
    pub shrink_upload_ring: bool,
    pub free_transients: bool,
    pub evict_idle_resources: bool,
}

pub fn pressure(local: MemoryBudget) -> Pressure {
    match local.budget {
        0 => Pressure::None,
        budget => match local.usage as f64 / budget as f64 {
            ratio if ratio >= 0.95 => Pressure::Hard,
            ratio if ratio >= 0.80 => Pressure::Soft,
            _ => Pressure::None,
        },
    }
}

pub fn trim_for(pressure: Pressure, visibility: Visibility) -> Trim {
    match (visibility, pressure) {
        (Visibility::Hidden, _) => Trim {
            shrink_upload_ring: true,
            free_transients: true,
            evict_idle_resources: true,
        },
        (Visibility::Visible, Pressure::Hard) => Trim {
            shrink_upload_ring: true,
            free_transients: true,
            evict_idle_resources: true,
        },
        (Visibility::Visible, Pressure::Soft) => Trim {
            shrink_upload_ring: true,
            free_transients: true,
            evict_idle_resources: false,
        },
        (Visibility::Visible, Pressure::None) => Trim::default(),
    }
}
