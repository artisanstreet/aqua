#[cfg(windows)]
pub(crate) use aqua_shell_win32 as shell;

#[cfg(target_os = "linux")]
#[path = "platform/wayland.rs"]
pub(crate) mod shell;

#[cfg(target_os = "macos")]
#[path = "platform/appkit.rs"]
pub(crate) mod shell;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) use aqua_shell::fallback as shell;
