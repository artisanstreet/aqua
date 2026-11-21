mod damage;
mod pacing;

#[cfg(target_os = "linux")]
mod occlusion;
#[cfg(target_os = "linux")]
mod partial;
#[cfg(target_os = "linux")]
mod present;
#[cfg(target_os = "linux")]
mod recovery;
#[cfg(target_os = "linux")]
mod resize;
#[cfg(target_os = "linux")]
mod support;
