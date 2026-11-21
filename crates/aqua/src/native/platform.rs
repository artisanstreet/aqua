#[cfg(windows)]
pub(crate) use aqua_shell_win32 as shell;

#[cfg(target_os = "linux")]
#[path = "platform/wayland.rs"]
pub(crate) mod shell;

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) use aqua_shell::fallback as shell;
