use std::panic::Location;
use std::sync::OnceLock;

pub type Pump = fn(&'static Location<'static>);

static PUMP: OnceLock<Pump> = OnceLock::new();

pub fn install(pump: Pump) -> bool {
    PUMP.set(pump).is_ok()
}

#[track_caller]
pub fn reach() {
    if let Some(pump) = PUMP.get() {
        pump(Location::caller());
    }
}
