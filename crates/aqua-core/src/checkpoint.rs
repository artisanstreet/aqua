use std::sync::OnceLock;

pub type Pump = fn();

static PUMP: OnceLock<Pump> = OnceLock::new();

pub fn install(pump: Pump) -> bool {
    PUMP.set(pump).is_ok()
}

pub fn reach() {
    if let Some(pump) = PUMP.get() {
        pump();
    }
}
