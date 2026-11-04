const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    Down,
    Up,
}

#[must_use]
pub fn key_phase(message: u32) -> Option<KeyPhase> {
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeyPhase::Down),
        WM_KEYUP | WM_SYSKEYUP => Some(KeyPhase::Up),
        _ => None,
    }
}

#[cfg(windows)]
pub use imp::Keystrokes;
#[cfg(windows)]
pub(crate) use imp::{enter_focus, leave_focus};

#[cfg(windows)]
mod imp {
    use super::{key_phase, KeyPhase};
    use std::cell::Cell;
    use windows::core::Interface;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::TextServices::{CLSID_TF_ThreadMgr, ITfKeystrokeMgr, ITfThreadMgr};

    thread_local! {
        static FOCUSED: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn enter_focus() {
        FOCUSED.with(|count| count.set(count.get() + 1));
    }

    pub(crate) fn leave_focus() {
        FOCUSED.with(|count| count.set(count.get().saturating_sub(1)));
    }

    fn focused() -> bool {
        FOCUSED.with(|count| count.get() > 0)
    }

    #[derive(Default)]
    pub struct Keystrokes {
        manager: Option<ITfKeystrokeMgr>,
    }

    impl Keystrokes {
        #[must_use]
        pub fn new() -> Self {
            Self::default()
        }

        fn manager(&mut self) -> Option<&ITfKeystrokeMgr> {
            if self.manager.is_none() {
                let thread: ITfThreadMgr =
                    unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }
                        .ok()?;
                self.manager = thread.cast().ok();
            }
            self.manager.as_ref()
        }

        pub fn pre_translate(&mut self, message: u32, wparam: usize, lparam: isize) -> bool {
            let Some(phase) = key_phase(message) else {
                return false;
            };
            if !focused() {
                return false;
            }
            let Some(manager) = self.manager() else {
                return false;
            };
            let (wparam, lparam) = (WPARAM(wparam), LPARAM(lparam));
            let eaten = unsafe {
                match phase {
                    KeyPhase::Down => manager.TestKeyDown(wparam, lparam),
                    KeyPhase::Up => manager.TestKeyUp(wparam, lparam),
                }
            };
            match eaten {
                Ok(eaten) if eaten.as_bool() => {
                    let handled = unsafe {
                        match phase {
                            KeyPhase::Down => manager.KeyDown(wparam, lparam),
                            KeyPhase::Up => manager.KeyUp(wparam, lparam),
                        }
                    };
                    handled.is_ok_and(|handled| handled.as_bool())
                }
                _ => false,
            }
        }
    }
}
