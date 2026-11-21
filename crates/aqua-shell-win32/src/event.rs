pub use aqua_shell::{
    Chrome, FileDrag, Handler, ShellError, ShellEvent, WindowKind, WindowOptions,
};

#[cfg(windows)]
pub(crate) fn win32(call: &'static str, error: windows::core::Error) -> ShellError {
    ShellError::Win32(call, error.code().0)
}
