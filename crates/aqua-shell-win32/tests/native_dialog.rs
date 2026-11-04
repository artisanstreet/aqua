#[cfg(windows)]
mod scripted {
    use aqua_protocol::{DialogKind, DialogSpec};
    use aqua_shell_win32::dialogs;
    use aqua_shell_win32::{Window, WindowOptions};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, FindWindowW, GetClassNameW, GetDlgCtrlID, PostMessageW, BM_CLICK,
        WM_COMMAND,
    };

    const IDOK: usize = 1;
    const IDCANCEL: usize = 2;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn find_dialog(title: &str, within: Duration) -> Option<HWND> {
        let title = wide(title);
        let started = Instant::now();
        while started.elapsed() < within {
            if let Ok(found) =
                unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) }
            {
                if !found.0.is_null() {
                    return Some(found);
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        None
    }

    fn find_button(dialog: HWND, id: usize) -> Option<HWND> {
        struct Search {
            id: i32,
            found: Option<HWND>,
        }
        unsafe extern "system" fn each(hwnd: HWND, state: LPARAM) -> windows::core::BOOL {
            let search = unsafe { &mut *(state.0 as *mut Search) };
            let mut class = [0u16; 32];
            let length = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
            let is_button = String::from_utf16_lossy(&class[..length]) == "Button";
            if is_button && unsafe { GetDlgCtrlID(hwnd) } == search.id {
                search.found = Some(hwnd);
                return false.into();
            }
            true.into()
        }
        let mut search = Search {
            id: id as i32,
            found: None,
        };
        let _ = unsafe { EnumChildWindows(Some(dialog), Some(each), LPARAM(&mut search as *mut _ as isize)) };
        search.found
    }

    fn script(title: &str, button: usize) -> std::thread::JoinHandle<bool> {
        let title = title.to_owned();
        std::thread::spawn(move || match find_dialog(&title, Duration::from_secs(20)) {
            Some(dialog) => {
                std::thread::sleep(Duration::from_millis(400));
                match find_button(dialog, button) {
                    Some(control) => {
                        unsafe { PostMessageW(Some(control), BM_CLICK, WPARAM(0), LPARAM(0)) }
                            .is_ok()
                    }
                    None => unsafe {
                        PostMessageW(Some(dialog), WM_COMMAND, WPARAM(button), LPARAM(0))
                    }
                    .is_ok(),
                }
            }
            None => false,
        })
    }

    fn pump() {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            let _ = unsafe { TranslateMessage(&message) };
            unsafe { DispatchMessageW(&message) };
        }
    }

    fn run_dialog(spec: &DialogSpec, button: usize) -> Vec<PathBuf> {
        let owner = Window::create(&WindowOptions {
            visible: false,
            ..Default::default()
        })
        .expect("owner");
        let driver = script(&spec.title, button);
        let job = dialogs::open(spec, Some(owner), owner.waker()).expect("open");
        let started = Instant::now();
        let result = loop {
            if let Some(result) = job.poll() {
                break result;
            }
            assert!(started.elapsed() < Duration::from_secs(30), "dialog hung");
            pump();
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(driver.join().expect("driver"), "dialog never appeared");
        owner.destroy();
        result.expect("dialog result")
    }

    pub fn save_confirm_returns_the_suggested_name() {
        let spec = DialogSpec {
            kind: DialogKind::SaveFile {
                suggested_name: "aqua-scripted-save.txt".into(),
            },
            title: "Aqua scripted save dialog".into(),
            filters: Vec::new(),
        };
        let chosen = run_dialog(&spec, IDOK);
        assert_eq!(chosen.len(), 1, "{chosen:?}");
        assert_eq!(
            chosen[0].file_name().and_then(|n| n.to_str()),
            Some("aqua-scripted-save.txt")
        );
        assert!(chosen[0].parent().is_some_and(|dir| dir.is_dir()));
        assert!(!chosen[0].exists(), "the dialog must not create the file");
    }

    pub fn save_cancel_returns_nothing() {
        let spec = DialogSpec {
            kind: DialogKind::SaveFile {
                suggested_name: "aqua-scripted-cancel.txt".into(),
            },
            title: "Aqua scripted cancel dialog".into(),
            filters: Vec::new(),
        };
        assert!(run_dialog(&spec, IDCANCEL).is_empty());
    }
}

fn main() {
    #[cfg(windows)]
    {
        if aqua_shell_win32::dialogs::run_helper_if_requested() {
            return;
        }
        scripted::save_confirm_returns_the_suggested_name();
        println!("test save_confirm_returns_the_suggested_name ... ok");
        scripted::save_cancel_returns_nothing();
        println!("test save_cancel_returns_nothing ... ok");
    }
}
