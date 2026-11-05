#[cfg(windows)]
mod probe {
    use std::time::{Duration, Instant};

    use aqua_core::geometry::Rect;
    use aqua_core::Oklch;
    use aqua_graph::{plan_paint, Extent, FramePlan, PaintCommand, Quad, Shadow, ShadowCast};
    use aqua_protocol::{
        FrameInputs, GpuBackend, NativeSurface, PresentHint, SurfaceId, SurfaceTarget, WindowId,
    };
    use windows::core::w;
    use windows::Win32::Foundation::{FILETIME, HWND, RECT};
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessTimes,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;

    const CLIENT: (i32, i32) = (1898, 1125);
    const FRAMES: usize = 600;

    pub struct Window {
        hwnd: HWND,
    }

    impl Window {
        fn open(width: i32, height: i32) -> Self {
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    w!("aqua-d3d-probe"),
                    WS_POPUP,
                    40,
                    40,
                    width,
                    height,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .expect("window");
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_BOTTOM),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            let window = Self { hwnd };
            window.pump();
            window
        }

        fn client(&self) -> Extent {
            let mut rect = RECT::default();
            unsafe { GetClientRect(self.hwnd, &mut rect).expect("client") };
            Extent {
                width: (rect.right - rect.left) as u32,
                height: (rect.bottom - rect.top) as u32,
            }
        }

        fn resize(&self, width: i32, height: i32) {
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    0,
                    0,
                    width,
                    height,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            self.pump();
        }

        fn pump(&self) {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }

        fn native(&self, extent: Extent) -> SurfaceTarget {
            SurfaceTarget::Native(NativeSurface {
                window: WindowId::new(1).unwrap(),
                hwnd: self.hwnd.0 as u64,
                extent,
            })
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }

    fn quad(x: f32, y: f32, w: f32, h: f32, hex: &str, radius: f32) -> PaintCommand {
        PaintCommand::Quad(Quad {
            bounds: Rect { x, y, w, h },
            radius,
            border_width: 0.0,
            fill: Oklch::from_hex(hex).unwrap(),
            border_color: Oklch::from_hex("#000000").unwrap(),
        })
    }

    pub fn scene(extent: Extent, phase: f32) -> FramePlan {
        let width = extent.width as f32;
        let height = extent.height as f32;
        let mut commands = vec![
            quad(0.0, 0.0, 280.0, height, "#1b1f27", 0.0),
            quad(280.0, 0.0, width - 280.0, 44.0, "#232833", 0.0),
        ];
        commands.extend((0..40).map(|row| {
            let y = 60.0 + row as f32 * 26.0 - phase * 26.0;
            quad(300.0, y, width - 340.0, 20.0, "#2c3340", 4.0)
        }));
        commands.push(PaintCommand::Shadow(Shadow {
            bounds: Rect {
                x: width * 0.5 - 200.0,
                y: height - 160.0,
                w: 400.0,
                h: 96.0,
            },
            radius: 12.0,
            offset: [0.0, 6.0],
            sigma: 12.0,
            spread: 0.0,
            color: Oklch::from_hex("#000000").unwrap().with_alpha(0.5),
            cast: ShadowCast::Whole,
        }));
        commands.push(PaintCommand::PushContentBlur {
            bounds: Rect {
                x: width * 0.5 - 200.0,
                y: height - 160.0,
                w: 400.0,
                h: 96.0,
            },
            radius: 8.0,
        });
        commands.push(quad(
            width * 0.5 - 190.0,
            height - 150.0,
            380.0,
            76.0,
            "#3b82f6",
            12.0,
        ));
        commands.push(PaintCommand::PopContentBlur);
        plan_paint(extent, Oklch::from_hex("#15181e").unwrap(), &commands).unwrap()
    }

    fn filetime(time: FILETIME) -> u64 {
        ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
    }

    fn process_times() -> (Duration, Duration) {
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        }
        .expect("process times");
        let since_epoch = Duration::from_nanos((filetime(created) - 116_444_736_000_000_000) * 100);
        let alive = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .saturating_sub(since_epoch);
        (
            alive,
            Duration::from_nanos((filetime(kernel) + filetime(user)) * 100),
        )
    }

    fn private_mib() -> f64 {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        let read = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            )
        };
        match read {
            Ok(()) => counters.PrivateUsage as f64 / 1048576.0,
            Err(_) => 0.0,
        }
    }

    fn dedicated_mib() -> f64 {
        let pid = unsafe { GetCurrentProcessId() };
        let script = format!(
            "$s = Get-Counter -Counter '\\GPU Process Memory(pid_{pid}*)\\Dedicated Usage' -ErrorAction Stop; ($s.CounterSamples | Measure-Object -Property CookedValue -Sum).Sum"
        );
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .ok()
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<f64>()
                    .ok()
            })
            .map_or(-1.0, |bytes| bytes / 1048576.0)
    }

    pub struct Sample {
        pub private_mib: f64,
        pub dxgi_local_mib: f64,
        pub counter_mib: f64,
    }

    impl std::fmt::Display for Sample {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "private_mib={:.1} dxgi_local_mib={:.1} gpu_dedicated_mib={:.1}",
                self.private_mib, self.dxgi_local_mib, self.counter_mib
            )
        }
    }

    fn sample(local_bytes: u64, window: &Window) -> Sample {
        let settle = Instant::now();
        while settle.elapsed() < Duration::from_millis(500) {
            window.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        Sample {
            private_mib: private_mib(),
            dxgi_local_mib: local_bytes as f64 / 1048576.0,
            counter_mib: dedicated_mib(),
        }
    }

    fn ms(duration: Duration) -> f64 {
        duration.as_secs_f64() * 1000.0
    }

    macro_rules! drive {
        ($name:literal, $backend:ty, $options:expr) => {{
            let started = Instant::now();
            let (process_alive, _) = process_times();
            <$backend>::begin_early($options, || None);
            let window = Window::open(CLIENT.0, CLIENT.1);
            let mut backend = <$backend>::take_early()
                .expect("early backend started")
                .expect("backend builds");
            let device_ms = ms(started.elapsed());
            let extent = window.client();
            let surface: SurfaceId = backend.create_surface(window.native(extent)).expect("surface");
            let frame = |backend: &mut $backend, extent: Extent, phase: f32| {
                backend.wait_for_frame_slot(surface).expect("slot");
                backend
                    .execute(&scene(extent, phase), &FrameInputs::new(surface))
                    .expect("execute");
                backend.present(surface, PresentHint::Vsync).expect("present");
            };
            frame(&mut backend, extent, 0.0);
            let first_present_ms = ms(started.elapsed());
            let first_present_process_ms = ms(process_alive) + first_present_ms;
            backend.finish_startup();
            while backend.startup_pending() {
                backend.startup_step();
                window.pump();
            }
            let startup = sample(
                backend.memory_breakdown().map_or(0, |memory| memory.local_usage),
                &window,
            );
            let (_, frames_cpu_before) = process_times();
            for index in 0..FRAMES {
                frame(&mut backend, extent, (index % 26) as f32 / 26.0);
                window.pump();
            }
            let (_, frames_cpu_after) = process_times();
            let frame_cpu_ms = ms(frames_cpu_after - frames_cpu_before) / FRAMES as f64;
            let steady = sample(
                backend.memory_breakdown().map_or(0, |memory| memory.local_usage),
                &window,
            );
            for step in 0..3 {
                window.resize(CLIENT.0 - 120 + 60 * step, CLIENT.1 - 80 + 40 * step);
                let extent = window.client();
                backend.resize_surface(surface, extent).expect("resize");
                (0..5).for_each(|index| frame(&mut backend, extent, index as f32 / 5.0));
                window.pump();
            }
            let (_, cpu_before) = process_times();
            let idle_started = Instant::now();
            while idle_started.elapsed() < Duration::from_secs(3) {
                window.pump();
                std::thread::sleep(Duration::from_millis(50));
            }
            let (_, cpu_after) = process_times();
            let idle_cpu = ms(cpu_after - cpu_before) / idle_started.elapsed().as_secs_f64();
            println!(
                "probe backend={} adapter=\"{}\" device_ms={device_ms:.1} first_present_ms={first_present_ms:.1} first_present_since_process_ms={first_present_process_ms:.1}",
                $name,
                backend.capabilities().adapter
            );
            println!("probe backend={} stage=startup {startup}", $name);
            println!("probe backend={} stage=after_{FRAMES}_frames {steady}", $name);
            println!("probe backend={} frame_cpu_ms={frame_cpu_ms:.3}", $name);
            println!(
                "probe backend={} idle_cpu_ms_per_s_after_resizes={idle_cpu:.2} pipelines={:?}",
                $name,
                backend.pipeline_stats()
            );
        }};
    }

    pub fn run() {
        let which = std::env::args().nth(1).unwrap_or_else(|| "d3d11".into());
        match which.as_str() {
            "d3d12" => drive!(
                "d3d12",
                aqua_gpu_d3d12::window::WindowBackend,
                aqua_gpu_d3d12::window::WindowOptions::from_env()
            ),
            _ => drive!(
                "d3d11",
                aqua_gpu_d3d11::window::WindowBackend,
                aqua_gpu_d3d11::window::WindowOptions::from_env()
            ),
        }
    }
}

fn main() {
    #[cfg(windows)]
    probe::run();
}
