#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    if aqua_shell_win32::dialogs::run_helper_if_requested() {
        return;
    }
    demo::main();
}

#[cfg(windows)]
mod demo {
    use aqua_core::{CursorIcon, InputEvent, Key, PointerButton};
    use aqua_protocol::WindowMaterial;
    use aqua_protocol::{CaptionControl, CaptionRegion, DragRegions, Instant, WindowId};
    use aqua_shell_win32::{
        clipboard, dialogs, now, prefs, quit, run, Chrome, Handler, ShellEvent, Window, WindowKind,
        WindowOptions,
    };
    use std::fmt::Write as _;
    use std::path::PathBuf;
    use std::time::Duration;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const TITLE_HEIGHT: f64 = 40.0;

    fn regions(width: f64) -> DragRegions {
        let button = |index: f64, control| CaptionRegion {
            control,
            bounds: (width - 138.0 + index * 46.0, 0.0, 46.0, TITLE_HEIGHT),
        };
        DragRegions {
            draggable: vec![(0.0, 0.0, width, TITLE_HEIGHT)],
            no_drag: vec![],
            caption_controls: vec![
                button(0.0, CaptionControl::Minimize),
                button(1.0, CaptionControl::Maximize),
                button(2.0, CaptionControl::Close),
            ],
        }
    }

    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t
    }

    fn pack_color(c: [f32; 3]) -> u32 {
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u32;
        0xFF00_0000 | (byte(c[0]) << 16) | (byte(c[1]) << 8) | byte(c[2])
    }

    fn fill(buffer: &mut [u32], width: u32, height: u32, rect: (i32, i32, i32, i32), color: u32) {
        let (x0, y0) = (
            rect.0.clamp(0, width as i32) as usize,
            rect.1.clamp(0, height as i32) as usize,
        );
        let (x1, y1) = (
            rect.2.clamp(0, width as i32) as usize,
            rect.3.clamp(0, height as i32) as usize,
        );
        (y0..y1).for_each(|y| {
            buffer[y * width as usize + x0..y * width as usize + x1.max(x0)].fill(color)
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        width: u32,
        height: u32,
        scale: f64,
        mouse: (f64, f64),
        hover: Option<CaptionControl>,
        pulse: f32,
        scroll: f32,
        typed: usize,
        panel: bool,
    ) -> Vec<u8> {
        let mut buffer = vec![0u32; (width * height) as usize];
        let px = |v: f64| (v * scale).round() as i32;
        let (w, h) = (width as i32, height as i32);
        let logical_width = f64::from(width) / scale;
        match panel {
            true => fill(
                &mut buffer,
                width,
                height,
                (0, 0, w, h),
                pack_color([0.95, 0.75, 0.2]),
            ),
            false => {
                for y in 0..h {
                    let t = y as f32 / h as f32;
                    let c = [
                        lerp(0.07, 0.12, t) + 0.25 * pulse,
                        lerp(0.09, 0.16, t) + 0.18 * pulse,
                        lerp(0.14, 0.26, t) + 0.05 * pulse,
                    ];
                    fill(&mut buffer, width, height, (0, y, w, y + 1), pack_color(c));
                }
                fill(
                    &mut buffer,
                    width,
                    height,
                    (0, 0, w, px(TITLE_HEIGHT)),
                    pack_color([0.04, 0.05, 0.08]),
                );
                let controls = [
                    CaptionControl::Minimize,
                    CaptionControl::Maximize,
                    CaptionControl::Close,
                ];
                for (k, control) in controls.into_iter().enumerate() {
                    let left = logical_width - 138.0 + k as f64 * 46.0;
                    let color = match (hover == Some(control), control) {
                        (true, CaptionControl::Close) => [0.8, 0.15, 0.15],
                        (true, _) => [0.22, 0.24, 0.32],
                        _ => [0.10, 0.11, 0.16],
                    };
                    fill(
                        &mut buffer,
                        width,
                        height,
                        (px(left), 0, px(left + 46.0), px(TITLE_HEIGHT)),
                        pack_color(color),
                    );
                }
                fill(
                    &mut buffer,
                    width,
                    height,
                    (
                        px(20.0),
                        px(60.0),
                        px(20.0 + f64::from(scroll.clamp(0.0, 400.0))),
                        px(76.0),
                    ),
                    pack_color([0.2, 0.8, 0.5]),
                );
                for n in 0..typed.min(30) {
                    let left = 20.0 + n as f64 * 14.0;
                    fill(
                        &mut buffer,
                        width,
                        height,
                        (px(left), px(90.0), px(left + 10.0), px(106.0)),
                        pack_color([0.9, 0.9, 0.95]),
                    );
                }
                fill(
                    &mut buffer,
                    width,
                    height,
                    (
                        px(mouse.0 - 8.0),
                        px(mouse.1 - 8.0),
                        px(mouse.0 + 8.0),
                        px(mouse.1 + 8.0),
                    ),
                    pack_color([1.0, 0.85, 0.2]),
                );
            }
        }
        buffer.into_iter().flat_map(u32::to_le_bytes).collect()
    }

    fn capture(window: Window, path: &PathBuf) -> bool {
        let hwnd = HWND(window.native_handle_hwnd() as *mut _);
        let (width, height) = window.inner_size();
        capture_hwnd(hwnd, width, height, path)
    }

    fn capture_hwnd(hwnd: HWND, width: u32, height: u32, path: &PathBuf) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, width as i32, height as i32);
            let old = SelectObject(dc, bitmap.into());
            let printed = PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(0x3)).as_bool();
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            GetDIBits(
                dc,
                bitmap,
                0,
                height,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            );
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            let mut file = Vec::with_capacity(pixels.len() + 54);
            file.extend_from_slice(b"BM");
            file.extend_from_slice(&((pixels.len() + 54) as u32).to_le_bytes());
            file.extend_from_slice(&[0, 0, 0, 0]);
            file.extend_from_slice(&54u32.to_le_bytes());
            file.extend_from_slice(&40u32.to_le_bytes());
            file.extend_from_slice(&(width as i32).to_le_bytes());
            file.extend_from_slice(&(-(height as i32)).to_le_bytes());
            file.extend_from_slice(&1u16.to_le_bytes());
            file.extend_from_slice(&32u16.to_le_bytes());
            file.extend_from_slice(&[0; 24]);
            file.extend_from_slice(&pixels);
            printed && std::fs::write(path, file).is_ok()
        }
    }

    fn post(window: Window, message: u32, wparam: usize, lparam: isize) {
        let _ = unsafe {
            PostMessageW(
                Some(HWND(window.native_handle_hwnd() as *mut _)),
                message,
                WPARAM(wparam),
                LPARAM(lparam),
            )
        };
    }

    fn send(window: Window, message: u32, wparam: usize, lparam: isize) -> isize {
        unsafe {
            SendMessageW(
                HWND(window.native_handle_hwnd() as *mut _),
                message,
                Some(WPARAM(wparam)),
                Some(LPARAM(lparam)),
            )
            .0
        }
    }

    fn pack(x: i32, y: i32) -> isize {
        ((y as u16 as isize) << 16) | (x as u16 as isize)
    }

    trait Raw {
        fn native_handle_hwnd(self) -> isize;
    }

    impl Raw for Window {
        fn native_handle_hwnd(self) -> isize {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match self.native_handle().window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
                _ => 0,
            }
        }
    }

    struct Probe {
        window: Window,
        size: (u32, u32),
        mouse: (f64, f64),
        hover: Option<CaptionControl>,
        scroll: f32,
        typed: usize,
        pulse_start: Option<Instant>,
        events: u32,
        ticks: u32,
        redraws: u32,
        wakes: u32,
        draw_nanos: u64,
        render_nanos: u64,
        draw_enabled: bool,
        tick_times: Vec<Instant>,
        log: String,
    }

    struct Demo {
        main: Probe,
        panel: Option<(Window, WindowId)>,
        material_window: Option<Window>,
        stats: bool,
        selftest: Option<SelfTest>,
        last_report: Instant,
    }

    struct SelfTest {
        out: PathBuf,
        step: usize,
        next: Instant,
        foreground_at_start: isize,
        dialog: Option<dialogs::DialogJob>,
        tries: u32,
    }

    fn owned_dialog(owner: isize) -> Option<HWND> {
        struct Search {
            owner: isize,
            found: Option<HWND>,
        }
        unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            let search = unsafe { &mut *(lparam.0 as *mut Search) };
            let mut class = [0u16; 16];
            let length = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
            let is_dialog = String::from_utf16_lossy(&class[..length]) == "#32770";
            let owned = unsafe { GetWindow(hwnd, GW_OWNER) }
                .map(|o| o.0 as isize == search.owner)
                .unwrap_or(false);
            if is_dialog && owned && unsafe { IsWindowVisible(hwnd) }.as_bool() {
                search.found = Some(hwnd);
                return false.into();
            }
            true.into()
        }
        let mut search = Search { owner, found: None };
        let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
        search.found
    }

    fn foreground() -> isize {
        unsafe { GetForegroundWindow().0 as isize }
    }

    impl Probe {
        fn pulse(&self, at: Instant) -> f32 {
            self.pulse_start.map_or(0.0, |start| {
                (1.0 - at.saturating_duration_since(start).as_secs_f32()).max(0.0)
            })
        }

        fn draw(&mut self, at: Instant, panel: bool) {
            let (width, height) = self.window.inner_size();
            self.size = (width, height);
            if width == 0 || height == 0 || !self.draw_enabled {
                return;
            }
            let started = std::time::Instant::now();
            let pixels = render(
                width,
                height,
                self.window.scale(),
                self.mouse,
                self.hover,
                self.pulse(at),
                self.scroll,
                self.typed,
                panel,
            );
            let rendered = started.elapsed();
            self.window.present_bgra(width, height, &pixels);
            self.draw_nanos += started.elapsed().as_nanos() as u64;
            self.render_nanos += rendered.as_nanos() as u64;
            self.redraws += 1;
        }

        fn line(&mut self, text: impl AsRef<str>) {
            let _ = writeln!(self.log, "{}", text.as_ref());
        }
    }

    impl Handler for Demo {
        fn window_event(&mut self, id: WindowId, event: ShellEvent) {
            self.main.events += 1;
            let is_main = id == self.main.window.id();
            let is_panel = self.panel.is_some_and(|(_, pid)| pid == id);
            match event {
                ShellEvent::Resized { .. } | ShellEvent::ScaleChanged { .. } if is_main => {
                    let width =
                        f64::from(self.main.window.inner_size().0) / self.main.window.scale();
                    self.main.window.set_drag_regions(&regions(width));
                    self.main.window.request_redraw();
                }
                ShellEvent::CloseRequested if is_main => self.main.window.destroy(),
                ShellEvent::CloseRequested => {}
                ShellEvent::CaptionHover(hover) if is_main => {
                    self.main.hover = hover;
                    self.main.line(format!("caption-hover {hover:?}"));
                    self.main.window.request_redraw();
                }
                ShellEvent::Redraw if is_main => self.main.draw(now(), false),
                ShellEvent::Redraw if is_panel => {
                    if let Some((panel, _)) = self.panel {
                        let mut probe = Probe {
                            window: panel,
                            ..self.blank()
                        };
                        probe.draw(now(), true);
                    }
                }
                ShellEvent::PreferencesChanged => self.main.line("preferences-changed"),
                ShellEvent::Focused(focused) => self.main.line(format!("focus {id:?} {focused}")),
                ShellEvent::Input(input) if is_main => self.input(input),
                _ => {}
            }
        }

        fn woken(&mut self) {
            self.main.wakes += 1;
        }

        fn tick(&mut self, at: Instant) -> Option<Instant> {
            self.main.ticks += 1;
            self.main.tick_times.push(at);
            if self.stats
                && at.saturating_duration_since(self.last_report) >= Duration::from_secs(1)
            {
                eprintln!(
                    "aqua-shell-stats events={} ticks={} redraws={} wakes={}",
                    self.main.events, self.main.ticks, self.main.redraws, self.main.wakes
                );
                self.last_report = at;
            }
            let animating = self.main.pulse(at) > 0.0;
            if !animating {
                self.main.pulse_start = None;
            } else {
                self.main.window.request_redraw();
            }
            let selftest_wake = self.selftest_tick(at);
            let pulse_wake = animating.then_some(at);
            let stats_wake = self
                .stats
                .then_some(self.last_report + Duration::from_secs(1));
            [selftest_wake, pulse_wake, stats_wake]
                .into_iter()
                .flatten()
                .min()
        }
    }

    impl Demo {
        fn blank(&self) -> Probe {
            Probe {
                window: self.main.window,
                size: (0, 0),
                mouse: (-100.0, -100.0),
                hover: None,
                scroll: 0.0,
                typed: 0,
                pulse_start: None,
                events: 0,
                ticks: 0,
                redraws: 0,
                wakes: 0,
                draw_nanos: 0,
                render_nanos: 0,
                draw_enabled: true,
                tick_times: Vec::new(),
                log: String::new(),
            }
        }

        fn input(&mut self, input: InputEvent) {
            match input {
                InputEvent::PointerMove(p) => {
                    self.main.mouse = (f64::from(p.position.x), f64::from(p.position.y));
                    self.main.window.request_redraw();
                }
                InputEvent::PointerDown(p, button) => {
                    self.main.line(format!(
                        "pointer-down {button:?} at ({:.1},{:.1}) buttons={}",
                        p.position.x, p.position.y, p.buttons
                    ));
                    if button == PointerButton::Primary && f64::from(p.position.y) >= TITLE_HEIGHT {
                        self.main.pulse_start = Some(now());
                        self.main.window.request_redraw();
                    }
                }
                InputEvent::PointerUp(p, button) => {
                    self.main
                        .line(format!("pointer-up {button:?} buttons={}", p.buttons));
                }
                InputEvent::Scroll { at, delta } => {
                    self.main.scroll += delta.y.abs() * 4.0;
                    self.main.line(format!(
                        "scroll at ({:.1},{:.1}) delta=({},{})",
                        at.x, at.y, delta.x, delta.y
                    ));
                    self.main.window.request_redraw();
                }
                InputEvent::KeyDown(key) => {
                    self.main.line(format!(
                        "key-down {:?} mods={:?} repeat={}",
                        key.key, key.modifiers, key.repeat
                    ));
                    if let Key::Char(_) = key.key {
                        self.main.typed += 1;
                        self.main.window.request_redraw();
                    }
                }
                InputEvent::TextInput { text } => self.main.line(format!("text {text:?}")),
                InputEvent::ModifiersChanged(m) => self.main.line(format!("modifiers {m:?}")),
                _ => {}
            }
        }

        fn selftest_tick(&mut self, at: Instant) -> Option<Instant> {
            let mut test = self.selftest.take()?;
            if let Some(job) = test.dialog.as_ref() {
                if let Some(result) = job.poll() {
                    self.main.line(format!("dialog-result {result:?}"));
                    test.dialog = None;
                }
            }
            if at < test.next {
                let next = test.next;
                self.selftest = Some(test);
                return Some(next);
            }
            let window = self.main.window;
            let scale = window.scale();
            let px = |v: f64| (v * scale).round() as i32;
            let (left, top, _, _) = window.outer_rect();
            let (width, _) = window.inner_size();
            let lw = f64::from(width) / scale;
            let delay = match test.step {
                0 => {
                    self.main.line(format!(
                        "scale={scale} client={:?} build={:?} dark={:?}",
                        window.inner_size(),
                        prefs::build_number(),
                        prefs::apps_use_dark_theme()
                    ));
                    self.main.line(format!("prefs={:?}", prefs::query()));
                    self.main.line(format!("monitor={:?}", window.monitor()));
                    let probes = [
                        ("caption-drag", 200.0, 20.0),
                        ("minimize", lw - 138.0 + 23.0, 20.0),
                        ("maximize", lw - 138.0 + 69.0, 20.0),
                        ("close", lw - 138.0 + 115.0, 20.0),
                        ("client", 300.0, 300.0),
                        ("left-edge", 1.0 / scale, 300.0),
                        ("bottom-right", lw - 0.5, 559.0),
                        ("top-edge", 300.0, 0.0),
                    ];
                    for (name, x, y) in probes {
                        let code = send(window, WM_NCHITTEST, 0, pack(left + px(x), top + px(y)));
                        self.main.line(format!("hittest {name} -> {code}"));
                    }
                    100
                }
                1 => {
                    post(window, WM_MOUSEMOVE, 0, pack(px(500.0), px(300.0)));
                    post(window, WM_LBUTTONDOWN, 1, pack(px(500.0), px(300.0)));
                    post(window, WM_LBUTTONUP, 0, pack(px(500.0), px(300.0)));
                    post(window, WM_RBUTTONDOWN, 2, pack(px(520.0), px(310.0)));
                    post(window, WM_RBUTTONUP, 0, pack(px(520.0), px(310.0)));
                    post(
                        window,
                        WM_MOUSEWHEEL,
                        (120u32 as usize) << 16,
                        pack(left + px(500.0), top + px(300.0)),
                    );
                    post(
                        window,
                        WM_MOUSEWHEEL,
                        ((-240i16) as u16 as usize) << 16,
                        pack(left + px(500.0), top + px(300.0)),
                    );
                    post(window, WM_KEYDOWN, 0x41, 0x001E_0001);
                    post(window, WM_CHAR, 'a' as usize, 0x001E_0001);
                    post(window, WM_KEYUP, 0x41, 0xC01E_0001u32 as isize);
                    post(window, WM_KEYDOWN, 0x25, 0x014B_0001);
                    post(window, WM_CHAR, 0, 0);
                    post(window, WM_CHAR, 0xD83D, 1);
                    post(window, WM_CHAR, 0xDE00, 1);
                    post(
                        window,
                        WM_NCMOUSEMOVE,
                        20,
                        pack(left + px(lw - 20.0), top + px(20.0)),
                    );
                    200
                }
                2 => {
                    let ok = capture(window, &test.out.join("shell-main.bmp"));
                    self.main.line(format!("capture main={ok}"));
                    100
                }
                3 => {
                    let mut options = WindowOptions::panel((220.0, 90.0));
                    options.title = "panel".into();
                    options.position = Some((left + 40, top + 40));
                    options.visible = false;
                    match Window::create(&options) {
                        Ok(panel) => {
                            panel.show();
                            let id = panel.id();
                            self.panel = Some((panel, id));
                            self.main.line(format!(
                                "panel created kind={:?} ex_noactivate={}",
                                WindowKind::Panel,
                                unsafe {
                                    GetWindowLongW(
                                        HWND(panel.native_handle_hwnd() as *mut _),
                                        GWL_EXSTYLE,
                                    )
                                } as u32
                                    & WS_EX_NOACTIVATE.0
                                    != 0
                            ));
                        }
                        Err(error) => self.main.line(format!("panel failed {error}")),
                    }
                    300
                }
                4 => {
                    if let Some((panel, _)) = self.panel {
                        post(panel, WM_LBUTTONDOWN, 1, pack(10, 10));
                        post(panel, WM_LBUTTONUP, 0, pack(10, 10));
                        let ok = capture(panel, &test.out.join("shell-panel.bmp"));
                        self.main.line(format!(
                            "panel capture={ok} panel_foreground={} main_foreground={} foreground_unchanged={}",
                            panel.is_foreground(),
                            window.is_foreground(),
                            foreground() == test.foreground_at_start
                        ));
                    }
                    100
                }
                5 => {
                    let mut options = WindowOptions {
                        title: "material".into(),
                        logical_size: (300.0, 200.0),
                        chrome: Chrome::Custom,
                        visible: false,
                        ..Default::default()
                    };
                    options.dark = true;
                    if let Ok(w) = Window::create(&options) {
                        let outcome = w.set_material(WindowMaterial::Mica);
                        self.main.line(format!(
                            "material {outcome:?} attribute={:?}",
                            w.backdrop_attribute()
                        ));
                        let outcome = w.set_material(WindowMaterial::Acrylic);
                        self.main.line(format!(
                            "material {outcome:?} attribute={:?}",
                            w.backdrop_attribute()
                        ));
                        let outcome = w.set_material(WindowMaterial::Solid);
                        self.main.line(format!(
                            "material {outcome:?} attribute={:?}",
                            w.backdrop_attribute()
                        ));
                        self.material_window = Some(w);
                    }
                    50
                }
                6 => {
                    let safe =
                        clipboard::is_empty() || matches!(clipboard::get_text(), Ok(Some(_)));
                    if safe {
                        let previous = clipboard::get_text().ok().flatten();
                        let set = clipboard::set_text(
                            window,
                            "aqua-shell \u{1F600} clipboard\nroundtrip",
                        );
                        let got = clipboard::get_text();
                        self.main.line(format!("clipboard set={set:?} get={got:?}"));
                        if let Some(previous) = previous {
                            let _ = clipboard::set_text(window, &previous);
                        }
                    } else {
                        self.main
                            .line("clipboard skipped: non-text content present");
                    }
                    100
                }
                7 => {
                    let spec = aqua_protocol::DialogSpec {
                        kind: aqua_protocol::DialogKind::OpenFile { multiple: false },
                        title: "aqua-shell dialog probe".into(),
                        filters: vec![aqua_protocol::DialogFilter {
                            label: "Text".into(),
                            extensions: vec!["txt".into()],
                        }],
                    };
                    match dialogs::open(&spec, Some(window), window.waker()) {
                        Ok(job) => {
                            test.dialog = Some(job);
                            self.main.line("dialog opened");
                        }
                        Err(error) => self.main.line(format!("dialog open failed {error}")),
                    }
                    300
                }
                8 => {
                    let owner = window.native_handle_hwnd();
                    match owned_dialog(owner) {
                        Some(dialog) if test.dialog.is_some() => {
                            let mut rect = windows::Win32::Foundation::RECT::default();
                            let _ = unsafe { GetWindowRect(dialog, &mut rect) };
                            let (w, h) = (
                                (rect.right - rect.left).max(0) as u32,
                                (rect.bottom - rect.top).max(0) as u32,
                            );
                            let ok = capture_hwnd(dialog, w, h, &test.out.join("shell-dialog.bmp"));
                            self.main.line(format!(
                                "dialog window found size={w}x{h} capture={ok} dialog_foreground={} main_foreground={} foreground_unchanged={}",
                                unsafe { GetForegroundWindow() } == dialog,
                                window.is_foreground(),
                                foreground() == test.foreground_at_start
                            ));
                            let _ = unsafe {
                                PostMessageW(Some(dialog), WM_CLOSE, WPARAM(0), LPARAM(0))
                            };
                            300
                        }
                        _ if test.tries < 60 && test.dialog.is_some() => {
                            test.tries += 1;
                            test.next = at + Duration::from_millis(100);
                            let next = test.next;
                            self.selftest = Some(test);
                            return Some(next);
                        }
                        _ => {
                            self.main.line("dialog window never appeared");
                            100
                        }
                    }
                }
                9 => {
                    window.set_logical_size(100.0, 100.0);
                    self.main.line(format!(
                        "min-size client={:?} logical_min=(480,320) scaled={:?}",
                        window.inner_size(),
                        (480.0 * scale, 320.0 * scale)
                    ));
                    window.set_logical_size(900.0, 560.0);
                    self.main
                        .line(format!("restored client={:?}", window.inner_size()));
                    self.main.pulse_start = Some(at);
                    self.main.draw_enabled = false;
                    self.main.tick_times.clear();
                    self.main.draw_nanos = 0;
                    self.main.render_nanos = 0;
                    self.main.ticks = 0;
                    self.main.redraws = 0;
                    1100
                }
                10 => {
                    self.main.draw_enabled = true;
                    let gaps: Vec<f64> = self
                        .main
                        .tick_times
                        .windows(2)
                        .map(|w| w[1].saturating_duration_since(w[0]).as_secs_f64() * 1000.0)
                        .collect();
                    let mean = gaps.iter().sum::<f64>() / gaps.len().max(1) as f64;
                    let max = gaps.iter().cloned().fold(0.0, f64::max);
                    let min = gaps.iter().cloned().fold(f64::MAX, f64::min);
                    self.main.line(format!(
                        "tick-gap-ms mean={mean:.2} min={min:.2} max={max:.2}"
                    ));
                    self.main.line(format!(
                        "pulse-1s ticks={} redraws={} draw_ms={} render_ms={}",
                        self.main.ticks,
                        self.main.redraws,
                        self.main.draw_nanos / 1_000_000,
                        self.main.render_nanos / 1_000_000
                    ));
                    self.main.ticks = 0;
                    self.main.redraws = 0;
                    self.main.wakes = 0;
                    3000
                }
                11 => {
                    self.main.line(format!(
                        "idle-3s ticks={} redraws={} wakes={} events_total={} mouse={:?}",
                        self.main.ticks,
                        self.main.redraws,
                        self.main.wakes,
                        self.main.events,
                        self.main.mouse
                    ));
                    if let Some(w) = self.material_window {
                        w.destroy();
                    }
                    if let Some((panel, _)) = self.panel {
                        panel.destroy();
                    }
                    self.main.line(format!(
                        "foreground_unchanged_at_end={}",
                        foreground() == test.foreground_at_start
                    ));
                    let _ = std::fs::write(test.out.join("shell-selftest.log"), &self.main.log);
                    window.destroy();
                    self.selftest = None;
                    return None;
                }
                _ => 100,
            };
            test.step += 1;
            test.next = at + Duration::from_millis(delay);
            let next = test.next;
            self.selftest = Some(test);
            Some(next)
        }
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        let selftest_dir = args
            .iter()
            .position(|a| a == "--selftest")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from);
        let foreground_at_start = foreground();
        let window = Window::create(&WindowOptions {
            title: "aqua-shell-win32".into(),
            logical_size: (900.0, 560.0),
            min_logical_size: Some((480.0, 320.0)),
            chrome: Chrome::Custom,
            visible: selftest_dir.is_none(),
            ..Default::default()
        })
        .expect("window");
        window.set_cursor(CursorIcon::Default);
        if selftest_dir.is_some() {
            window.show_inactive();
            window.send_to_back();
        }
        let start = now();
        let main = Probe {
            window,
            size: (0, 0),
            mouse: (-100.0, -100.0),
            hover: None,
            scroll: 0.0,
            typed: 0,
            pulse_start: None,
            events: 0,
            ticks: 0,
            redraws: 0,
            wakes: 0,
            draw_nanos: 0,
            render_nanos: 0,
            draw_enabled: true,
            tick_times: Vec::new(),
            log: String::new(),
        };
        let demo = Demo {
            main,
            panel: None,
            material_window: None,
            stats: std::env::var_os("AQUA_FRAME_STATS").is_some(),
            selftest: selftest_dir.map(|out| SelfTest {
                out,
                step: 0,
                next: start + Duration::from_millis(500),
                foreground_at_start,
                dialog: None,
                tries: 0,
            }),
            last_report: start,
        };
        if run(demo).is_err() {
            quit();
        }
    }
}
