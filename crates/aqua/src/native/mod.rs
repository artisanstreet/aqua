#[cfg(windows)]
mod d3d12;
#[cfg(windows)]
mod hook;
#[cfg(windows)]
mod inspector;
#[cfg(windows)]
mod looping;
#[cfg(windows)]
mod window;

#[cfg(windows)]
pub use looping::run;

#[cfg(not(windows))]
pub fn run(
    _aqua: crate::runtime::Aqua,
    _app: impl crate::runtime::Application + 'static,
) -> Result<(), String> {
    Err("the native host runs on Windows only".into())
}
