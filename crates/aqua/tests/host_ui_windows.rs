#[cfg(windows)]
mod native {
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use aqua::config::AquaConfig;
    use aqua::{px, Aqua, Style};
    use aqua_core::Size;
    use aqua_protocol::{AquaWindowId, WindowSpec};
    use aqua_reactive::Scope;
    use aqua_runtime::{Runtime, RuntimeError};
    use aqua_ui::element::Element;
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
        TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_InvokePatternId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

    fn page(label: &'static str, count: u32, press: impl Fn() + 'static) -> Element {
        Aqua::Box()
            .style(Style::new().column().width(px(360.0)).height(px(200.0)))
            .child(Aqua::Text(format!("{label} count {count}")).build())
            .child(
                Aqua::Button()
                    .style(Style::new().width(px(180.0)).height(px(40.0)))
                    .on_activate(press)
                    .child(Aqua::Text(format!("Press {label}")).build())
                    .build(),
            )
            .build()
    }

    fn mount_page(runtime: &mut Runtime, label: &'static str) -> Result<(), RuntimeError> {
        let (count, set_count) = runtime.scope().signal(0u32);
        runtime.mount_reactive((count,), move |(count,)| {
            let set_count = set_count.clone();
            page(label, count, move || set_count.update(|count| *count += 1))
        })
    }

    fn root(_scope: &Scope) -> Element {
        page("first", 0, || {})
    }

    fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> Result<T, String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(value) = probe() {
                return Ok(value);
            }
            if Instant::now() > deadline {
                return Err(format!("timed out waiting for {what}"));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn find_window(title: windows::core::PCWSTR) -> Option<HWND> {
        unsafe { FindWindowW(None, title) }
            .ok()
            .filter(|hwnd| !hwnd.0.is_null())
    }

    fn elements(
        automation: &IUIAutomation,
        hwnd: HWND,
    ) -> Vec<(i32, String, IUIAutomationElement)> {
        unsafe {
            let Ok(root) = automation.ElementFromHandle(hwnd) else {
                return Vec::new();
            };
            let Ok(condition) = automation.CreateTrueCondition() else {
                return Vec::new();
            };
            let Ok(all) = root.FindAll(TreeScope_Descendants, &condition) else {
                return Vec::new();
            };
            (0..all.Length().unwrap_or(0))
                .filter_map(|index| {
                    let element = all.GetElement(index).ok()?;
                    let kind = element.CurrentControlType().ok()?.0;
                    let name = element.CurrentName().ok()?.to_string();
                    Some((kind, name, element))
                })
                .collect()
        }
    }

    fn has_name(automation: &IUIAutomation, hwnd: HWND, wanted: &str) -> bool {
        elements(automation, hwnd)
            .iter()
            .any(|(_, name, _)| name == wanted)
    }

    fn client() -> Result<(), String> {
        let first = wait_for("first window", || find_window(w!("aqua ui windows first")))?;
        let second = wait_for("second window", || {
            find_window(w!("aqua ui windows second"))
        })?;
        if first == second {
            return Err("both windows share a handle".into());
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| e.to_string())?;
        wait_for("first window UIA tree", || {
            has_name(&automation, first, "first count 0").then_some(())
        })?;
        wait_for("second window UIA tree", || {
            has_name(&automation, second, "second count 0").then_some(())
        })?;
        if has_name(&automation, first, "second count 0")
            || has_name(&automation, second, "first count 0")
        {
            return Err("a window exposes the other window's tree".into());
        }
        let button = wait_for("second window button", || {
            elements(&automation, second)
                .into_iter()
                .find(|(kind, name, _)| {
                    *kind == UIA_ButtonControlTypeId.0 && name == "Press second"
                })
                .map(|(_, _, element)| element)
        })?;
        let rect = unsafe { button.CurrentBoundingRectangle() }.map_err(|e| e.to_string())?;
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return Err(format!("empty UIA bounds {rect:?}"));
        }
        let invoke: IUIAutomationInvokePattern =
            unsafe { button.GetCurrentPatternAs(UIA_InvokePatternId) }
                .map_err(|e| e.to_string())?;
        unsafe { invoke.Invoke() }.map_err(|e| e.to_string())?;
        wait_for("the second window counter", || {
            has_name(&automation, second, "second count 1").then_some(())
        })?;
        if !has_name(&automation, first, "first count 0") {
            return Err("invoking the second window changed the first".into());
        }

        unsafe { PostMessageW(Some(second), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            .map_err(|e| e.to_string())?;
        wait_for("second window gone", || {
            find_window(w!("aqua ui windows second"))
                .is_none()
                .then_some(())
        })?;
        wait_for("first window still served", || {
            has_name(&automation, first, "first count 0").then_some(())
        })?;
        unsafe { PostMessageW(Some(first), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn run() {
        std::env::set_var("AQUA_SMOKE_MS", "90000");
        std::env::set_var("AQUA_RENDER_WHEN_COVERED", "1");
        let (done, outcome) = mpsc::channel();
        thread::spawn(move || {
            let _ = done.send(client());
        });
        let mut config = AquaConfig::default();
        config.window.title = "aqua ui windows first".into();
        let aqua = Aqua::new(config).expect("aqua");
        let fonts = aqua.fonts().clone();
        let runs = aqua.runs().clone();
        let second = WindowSpec {
            title: "aqua ui windows second".into(),
            size: Size { w: 360.0, h: 200.0 },
            ..WindowSpec::default()
        };
        let result = aqua.run_ui_windows_opening([second], root, move |_: AquaWindowId| {
            let mut runtime = Runtime::new(fonts.clone(), runs.clone());
            mount_page(&mut runtime, "second")?;
            Ok(runtime)
        });
        if let Err(error) = result {
            eprintln!("host_ui_windows: run failed: {error}");
            std::process::exit(1);
        }
        match outcome.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => eprintln!("host_ui_windows: ok"),
            Ok(Err(message)) => {
                eprintln!("host_ui_windows: FAILED: {message}");
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!("host_ui_windows: FAILED: client did not finish");
                std::process::exit(1);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    native::run();
    #[cfg(not(windows))]
    eprintln!("host_ui_windows: skipped; requires Windows");
}
