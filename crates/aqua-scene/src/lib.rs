#![forbid(unsafe_code)]

pub mod damage;
pub mod scene;

pub use damage::{DamageSet, DamageTracker, InvalidationReason};
pub use scene::{inflate, Scene};
