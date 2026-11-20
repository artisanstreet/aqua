#[cfg(windows)]
pub(crate) use aqua_shell_win32 as shell;

#[cfg(not(windows))]
pub(crate) use aqua_shell::fallback as shell;
