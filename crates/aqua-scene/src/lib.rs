#![forbid(unsafe_code)]

pub mod damage;
pub mod footprint;
pub mod scene;

pub use damage::{DamageSet, DamageTracker, InvalidationReason};
pub use footprint::Footprints;
pub use scene::{inflate, Scene};
