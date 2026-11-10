#[cfg(windows)]
#[path = "../../aqua-verify/tests/support/desktop.rs"]
mod desktop;

#[cfg(windows)]
mod native {
    use std::process::Command;
    use std::time::Duration;

    use aqua::config::AquaConfig;
    use aqua::runtime::{AppCtx, Application, Aqua};
    use aqua_core::{DisplayItem, InputEvent, NodeId, Oklch, Rect};
    use aqua_protocol::geometry::AquaWindowId;
    use aqua_protocol::{Instant, ObservedWindowState, WindowEvent, WindowMaterial};
    use aqua_scene::Scene;
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
        GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetForegroundWindow, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE,
    };

    const MATERIALS: [(&str, WindowMaterial, i32); 3] = [
        ("mica", WindowMaterial::Mica, 2),
        ("mica-alt", WindowMaterial::Tabbed, 4),
        ("acrylic", WindowMaterial::Acrylic, 3),
    ];
    const QUAD: &str = "#20c0ff";
    const WARM_FRAMES: u64 = 40;
    const SETTLE: Duration = Duration::from_millis(1500);

    fn quad_at(build: u64) -> DisplayItem {
        DisplayItem::Rect {
            bounds: Rect {
                x: 150.0 + (build % 2) as f32,
                y: 100.0,
                w: 100.0,
                h: 100.0,
            },
            radius: 0.0,
            fill: Oklch::from_hex(QUAD).unwrap(),
            border_width: 0.0,
            border_color: Oklch::TRANSPARENT,
            shadow: None,
        }
    }

    struct Frame {
        width: i32,
        bgra: Vec<u8>,
    }

    impl Frame {
        fn capture(x: i32, y: i32, width: i32, height: i32) -> Frame {
            unsafe {
                let screen = GetDC(None);
                let memory = CreateCompatibleDC(Some(screen));
                let bitmap = CreateCompatibleBitmap(screen, width, height);
                let previous = SelectObject(memory, bitmap.into());
                let copied = BitBlt(
                    memory,
                    0,
                    0,
                    width,
                    height,
                    Some(screen),
                    x,
                    y,
                    SRCCOPY | CAPTUREBLT,
                );
                let mut info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bgra = vec![0u8; (width * height * 4) as usize];
                GetDIBits(
                    memory,
                    bitmap,
                    0,
                    height as u32,
                    Some(bgra.as_mut_ptr().cast()),
                    &mut info,
                    DIB_RGB_COLORS,
                );
                SelectObject(memory, previous);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(memory);
                ReleaseDC(None, screen);
                copied.expect("screen capture");
                Frame { width, bgra }
            }
        }

        fn mean(&self, x: i32, y: i32, width: i32, height: i32) -> [f64; 3] {
            let mut sum = [0.0f64; 3];
            for row in y..y + height {
                for column in x..x + width {
                    let at = ((row * self.width + column) * 4) as usize;
                    sum[0] += f64::from(self.bgra[at + 2]);
                    sum[1] += f64::from(self.bgra[at + 1]);
                    sum[2] += f64::from(self.bgra[at]);
                }
            }
            let count = f64::from(width * height);
            sum.map(|channel| channel / count)
        }
    }

    fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
        a.iter().zip(b).map(|(a, b)| (a - b).abs()).sum::<f64>() / 3.0
    }

    struct Probe {
        scene: Scene,
        quad: NodeId,
        builds: u64,
        frames: u64,
        settled_at: Option<Instant>,
        out: String,
    }

    impl Probe {
        fn measure(&self, id: AquaWindowId) -> Result<String, String> {
            let hwnd = HWND(id.raw() as *mut _);
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            let mut client = RECT::default();
            unsafe { GetClientRect(hwnd, &mut client) }.map_err(|e| e.to_string())?;
            let mut origin = POINT { x: 0, y: 0 };
            if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
                return Err("ClientToScreen failed".into());
            }
            let (width, height) = (client.right, client.bottom);
            let shot = Frame::capture(origin.x, origin.y, width, height);
            let sample = width / 8;
            let band = height / 4;
            let left = shot.mean(sample, band, sample, band);
            let right = shot.mean(width - 2 * sample, band * 2, sample, band);
            let quad = shot.mean(width / 2 - 20, height / 2 - 20, 40, 40);
            let mut backdrop = 0i32;
            unsafe {
                DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    (&mut backdrop as *mut i32).cast(),
                    std::mem::size_of::<i32>() as u32,
                )
            }
            .map_err(|e| e.to_string())?;
            let active = u8::from(unsafe { GetForegroundWindow() } == hwnd);
            Ok(format!("{left:?} {right:?} {quad:?} {backdrop} {active}"))
        }
    }

    impl Application for Probe {
        fn on_start(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId) {}

        fn on_window_event(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &WindowEvent) {}

        fn on_input(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &InputEvent) {}

        fn build_scene(&mut self, _window: &ObservedWindowState, now: Instant) -> Option<Instant> {
            self.builds += 1;
            self.scene.update(self.quad, quad_at(self.builds));
            Some(now + Duration::from_millis(16))
        }

        fn scene_mut(&mut self, _id: AquaWindowId) -> Option<&mut Scene> {
            Some(&mut self.scene)
        }

        fn on_frame_completed(&mut self, ctx: &mut AppCtx, id: AquaWindowId, _sequence: u64) {
            self.frames += 1;
            if self.frames < WARM_FRAMES {
                return;
            }
            let now = ctx.now();
            let since = *self.settled_at.get_or_insert(now);
            if now.saturating_duration_since(since) < SETTLE {
                return;
            }
            let verdict = self.measure(id);
            let line = verdict.unwrap_or_else(|error| format!("ERROR {error}"));
            let _ = std::fs::write(&self.out, line);
            ctx.quit();
        }

        fn on_wake(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId) {}
    }

    fn child(name: &str, material: WindowMaterial, out: String) {
        std::env::set_var("AQUA_RENDER_WHEN_COVERED", "1");
        std::env::set_var("AQUA_SMOKE_MS", "20000");
        let mut config = AquaConfig::default();
        config.window.title = format!("aqua host material {name}");
        config.window.inner_size = aqua_protocol::geometry::LogicalSize::new(400.0, 300.0);
        config.material = material;
        let mut scene = Scene::new();
        let quad = scene.insert(quad_at(0));
        let probe = Probe {
            scene,
            quad,
            builds: 0,
            frames: 0,
            settled_at: None,
            out,
        };
        if let Err(error) = Aqua::new(config).expect("aqua").run(probe) {
            eprintln!("host_material {name}: run failed: {error}");
            std::process::exit(1);
        }
    }

    fn parse(line: &str) -> Result<([[f64; 3]; 3], i32, bool), String> {
        let numbers: Vec<f64> = line
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse().ok())
            .collect();
        match numbers.as_slice() {
            [a, b, c, d, e, f, g, h, i, backdrop, active] => Ok((
                [[*a, *b, *c], [*d, *e, *f], [*g, *h, *i]],
                *backdrop as i32,
                *active > 0.0,
            )),
            _ => Err(format!("unparseable measurement {line:?}")),
        }
    }

    struct Measured {
        backdrop: i32,
        active: bool,
        left: [f64; 3],
        right: [f64; 3],
        quad: [f64; 3],
    }

    fn run_child(name: &str) -> Result<Measured, String> {
        let out = std::env::temp_dir().join(format!("aqua-host-material-{name}.txt"));
        let _ = std::fs::remove_file(&out);
        let status = Command::new(std::env::current_exe().unwrap())
            .env("AQUA_HOST_MATERIAL", name)
            .env("AQUA_HOST_MATERIAL_OUT", &out)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("{name}: host exited with {status}"));
        }
        let line =
            std::fs::read_to_string(&out).map_err(|e| format!("{name}: no measurement: {e}"))?;
        let ([left, right, quad], backdrop, active) = parse(&line)?;
        println!(
        "{name}: backdrop attribute {backdrop} active {active} left {left:?} right {right:?} quad {quad:?}"
    );
        Ok(Measured {
            backdrop,
            active,
            left,
            right,
            quad,
        })
    }

    fn expected_quad() -> [f64; 3] {
        let rgb = Oklch::from_hex(QUAD).unwrap().to_srgb8();
        [f64::from(rgb[0]), f64::from(rgb[1]), f64::from(rgb[2])]
    }

    use super::desktop;

    pub fn run() {
        if let Ok(name) = std::env::var("AQUA_HOST_MATERIAL") {
            let material = MATERIALS
                .iter()
                .find(|(label, _, _)| *label == name)
                .map(|(_, material, _)| *material)
                .expect("known material");
            child(
                &name,
                material,
                std::env::var("AQUA_HOST_MATERIAL_OUT").unwrap(),
            );
            return;
        }
        if !desktop::require("host_material") {
            return;
        }
        let results: Vec<(&str, Result<Measured, String>)> = MATERIALS
            .iter()
            .map(|(name, _, _)| (*name, run_child(name)))
            .collect();
        let mut failures = Vec::new();
        for ((name, result), (_, _, attribute)) in results.iter().zip(MATERIALS.iter()) {
            match result {
                Err(error) => failures.push(format!("{name}: {error}")),
                Ok(m) => {
                    if m.backdrop != *attribute {
                        failures.push(format!(
                            "{name}: DWM system backdrop type {} instead of {attribute}",
                            m.backdrop
                        ));
                    }
                    if m.active {
                        println!("{name}: window was active, backdrop shows the live material");
                    }
                    [("left", m.left), ("right", m.right)]
                        .iter()
                        .filter(|(_, mean)| distance(*mean, [0.0; 3]) <= 3.0)
                        .for_each(|(side, mean)| {
                            failures.push(format!("{name}: {side} backdrop is black, got {mean:?}"))
                        });
                    if distance(m.quad, expected_quad()) > 6.0 {
                        failures.push(format!(
                            "{name}: opaque quad {:?} differs from {:?}",
                            m.quad,
                            expected_quad()
                        ));
                    }
                }
            }
        }
        match failures.is_empty() {
            true => eprintln!("host_material: ok"),
            false => {
                eprintln!("host_material: FAILED:\n{}", failures.join("\n"));
                std::process::exit(1);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    native::run();
    #[cfg(not(windows))]
    eprintln!("host_material: skipped; requires Windows");
}
