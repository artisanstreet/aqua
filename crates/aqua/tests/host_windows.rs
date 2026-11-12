#[cfg(windows)]
mod native {
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant as WallInstant};

    use aqua::config::AquaConfig;
    use aqua::runtime::{AppCtx, Application, Aqua};
    use aqua_core::InputEvent;
    use aqua_protocol::geometry::AquaWindowId;
    use aqua_protocol::window::{Titlebar, WindowOptions, WindowRequest};
    use aqua_protocol::{CaptionControl, Instant, ObservedWindowState, WindowEvent};
    use windows::core::w;
    use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetClientRect, GetWindowRect, PostMessageW, SendMessageW, HTMAXBUTTON,
        WM_CLOSE, WM_DPICHANGED, WM_NCMOUSEMOVE,
    };

    #[derive(Default)]
    struct Log {
        events: Vec<(AquaWindowId, String)>,
        hovers: Vec<(AquaWindowId, Option<CaptionControl>)>,
        started: Vec<AquaWindowId>,
    }

    struct Recorder {
        log: Arc<Mutex<Log>>,
    }

    impl Application for Recorder {
        fn on_start(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
            let first = {
                let mut log = self.log.lock().unwrap();
                log.started.push(id);
                log.started.len() == 1
            };
            if first {
                let options = WindowOptions {
                    title: "aqua mw second".into(),
                    titlebar: Titlebar::Overlay,
                    ..WindowOptions::default()
                };
                ctx.request_window(WindowRequest::Create(options)).unwrap();
            }
        }

        fn on_caption_hover(
            &mut self,
            _ctx: &mut AppCtx,
            id: AquaWindowId,
            control: Option<CaptionControl>,
        ) {
            self.log.lock().unwrap().hovers.push((id, control));
        }

        fn on_window_event(&mut self, _ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
            let label = match event {
                WindowEvent::ScaleChanged(_, scale) => format!("scale {scale}"),
                WindowEvent::Resized(_, size) => format!("resized {}x{}", size.width, size.height),
                WindowEvent::Destroyed(_) => "destroyed".to_string(),
                WindowEvent::CloseRequested(_) => "close".to_string(),
                _ => return,
            };
            self.log.lock().unwrap().events.push((id, label));
        }

        fn on_input(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &InputEvent) {}

        fn build_scene(&mut self, _window: &ObservedWindowState, _now: Instant) -> Option<Instant> {
            None
        }
    }

    fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> Result<T, String> {
        let deadline = WallInstant::now() + Duration::from_secs(20);
        loop {
            if let Some(value) = probe() {
                return Ok(value);
            }
            if WallInstant::now() > deadline {
                return Err(format!("timed out waiting for {what}"));
            }
            thread::sleep(Duration::from_millis(30));
        }
    }

    fn client(log: Arc<Mutex<Log>>) -> Result<(), String> {
        let first = wait_for("first window", || {
            unsafe { FindWindowW(None, w!("aqua mw first")) }
                .ok()
                .filter(|hwnd| !hwnd.0.is_null())
        })?;
        let second = wait_for("second window", || {
            unsafe { FindWindowW(None, w!("aqua mw second")) }
                .ok()
                .filter(|hwnd| !hwnd.0.is_null())
        })?;
        if first == second {
            return Err("both windows share a handle".into());
        }
        let second_id = second.0 as usize as u64;
        let first_id = first.0 as usize as u64;

        let mut before = RECT::default();
        unsafe { GetWindowRect(second, &mut before) }.map_err(|e| e.to_string())?;
        let suggested = RECT {
            left: before.left,
            top: before.top,
            right: before.left + (before.right - before.left) * 3 / 2,
            bottom: before.top + (before.bottom - before.top) * 3 / 2,
        };
        unsafe {
            SendMessageW(
                second,
                WM_DPICHANGED,
                Some(WPARAM(144 | (144 << 16))),
                Some(LPARAM(&suggested as *const RECT as isize)),
            );
        }
        wait_for("scale change on the second window only", || {
            let log = log.lock().unwrap();
            let scaled = log
                .events
                .iter()
                .any(|(id, label)| id.raw() == second_id && label == "scale 1.5");
            let resized_after = log
                .events
                .iter()
                .skip_while(|(_, label)| label != "scale 1.5")
                .any(|(id, label)| id.raw() == second_id && label.starts_with("resized"));
            (scaled && resized_after).then_some(())
        })?;
        if log
            .lock()
            .unwrap()
            .events
            .iter()
            .any(|(id, label)| id.raw() == first_id && label.starts_with("scale"))
        {
            return Err("DPI change leaked to the first window".into());
        }
        let mut client = RECT::default();
        unsafe { GetClientRect(second, &mut client) }.map_err(|e| e.to_string())?;
        if client.right <= 0 {
            return Err("second window has no client area after DPI change".into());
        }

        unsafe {
            SendMessageW(
                second,
                WM_NCMOUSEMOVE,
                Some(WPARAM(HTMAXBUTTON as usize)),
                Some(LPARAM(0)),
            );
        }
        wait_for("caption hover forwarded", || {
            log.lock()
                .unwrap()
                .hovers
                .iter()
                .any(|(id, control)| {
                    id.raw() == second_id && *control == Some(CaptionControl::Maximize)
                })
                .then_some(())
        })?;

        unsafe { PostMessageW(Some(second), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            .map_err(|e| e.to_string())?;
        wait_for("second window gone", || {
            unsafe { FindWindowW(None, w!("aqua mw second")) }
                .map_or(true, |hwnd| hwnd.0.is_null())
                .then_some(())
        })?;
        if !log
            .lock()
            .unwrap()
            .events
            .iter()
            .any(|(id, label)| id.raw() == second_id && label == "close")
        {
            return Err("the app never saw the close request".into());
        }
        if !log
            .lock()
            .unwrap()
            .events
            .iter()
            .any(|(id, label)| id.raw() == second_id && label == "destroyed")
        {
            return Err("the app never saw the window destroyed".into());
        }
        if unsafe { FindWindowW(None, w!("aqua mw first")) }
            .map(|hwnd| hwnd.0.is_null())
            .unwrap_or(true)
        {
            return Err("closing the second window closed the first".into());
        }
        unsafe { PostMessageW(Some(first), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn run() {
        unsafe { std::env::set_var("AQUA_SMOKE_MS", "60000") };
        unsafe { std::env::set_var("AQUA_RENDER_WHEN_COVERED", "1") };
        let log = Arc::new(Mutex::new(Log::default()));
        let (done, outcome) = std::sync::mpsc::channel();
        let shared = log.clone();
        thread::spawn(move || {
            let _ = done.send(client(shared));
        });
        let mut config = AquaConfig::default();
        config.window.title = "aqua mw first".into();
        let aqua = Aqua::new(config).expect("aqua");
        let result = aqua.run(Recorder { log: log.clone() });
        if let Err(error) = result {
            eprintln!("host_windows: run failed: {error}");
            std::process::exit(1);
        }
        match outcome.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => eprintln!("host_windows: ok"),
            Ok(Err(message)) => {
                eprintln!("host_windows: FAILED: {message}");
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!("host_windows: FAILED: client did not finish");
                std::process::exit(1);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    native::run();
    #[cfg(not(windows))]
    eprintln!("host_windows: skipped; requires Windows");
}
