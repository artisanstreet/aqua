#[cfg(windows)]
mod native {
use std::num::NonZeroUsize;
use std::process::Command;
use std::time::Duration;

use aqua::config::AquaConfig;
use aqua::mailbox::Mailbox;
use aqua::runtime::{AppCtx, Application, Aqua};
use aqua::UiApp;
use aqua_core::{InputEvent, Oklch, Transform2D};
use aqua_motion::Easing;
use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
use aqua_protocol::{Instant, ObservedWindowState, WindowEvent, WindowRequest};
use aqua_reactive::WriteSignal;
use aqua_scene::Scene;
use aqua_ui::element::ui::Box as UiBox;
use aqua_ui::element::Element;
use aqua_ui::motion::{Motion, Presence};
use aqua_ui::style::{px, Axis, Style};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};

const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 360.0;
const SLIDE: f32 = 60.0;
const ROW_X: f32 = 340.0;
const ROW_Y: f32 = 210.0;
const ROW_W: f32 = 90.0;
const ROW_H: f32 = 24.0;
const MOTION: Motion = Motion::tween(0.4, Easing::Linear);
const WARM: Duration = Duration::from_millis(700);
const WINDOW: Duration = Duration::from_millis(1100);

struct Shot {
    width: i32,
    scale: f32,
    bgra: Vec<u8>,
}

#[link(name = "user32")]
extern "system" {
    fn PrintWindow(hwnd: HWND, hdc: HDC, flags: u32) -> i32;
}

impl Shot {
    fn take(hwnd: HWND) -> Result<Shot, String> {
        unsafe {
            let mut outer = RECT::default();
            GetWindowRect(hwnd, &mut outer).map_err(|e| e.to_string())?;
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client).map_err(|e| e.to_string())?;
            let mut origin = POINT { x: 0, y: 0 };
            if !ClientToScreen(hwnd, &mut origin).as_bool() {
                return Err("ClientToScreen failed".into());
            }
            let (full_width, full_height) = (outer.right - outer.left, outer.bottom - outer.top);
            let (dx, dy) = (origin.x - outer.left, origin.y - outer.top);
            let (width, height) = (client.right, client.bottom);
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, full_width, full_height);
            let previous = SelectObject(memory, bitmap.into());
            let printed = PrintWindow(hwnd, memory, 2);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: full_width,
                    biHeight: -full_height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut full = vec![0u8; (full_width * full_height * 4) as usize];
            GetDIBits(
                memory,
                bitmap,
                0,
                full_height as u32,
                Some(full.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            );
            SelectObject(memory, previous);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            if printed == 0 {
                return Err("PrintWindow failed".into());
            }
            let mut bgra = Vec::with_capacity((width * height * 4) as usize);
            for row in 0..height {
                let start = (((row + dy) * full_width + dx) * 4) as usize;
                bgra.extend_from_slice(&full[start..start + (width * 4) as usize]);
            }
            Ok(Shot {
                width,
                scale: width as f32 / WIDTH,
                bgra,
            })
        }
    }

    fn at(&self, x: f32, y: f32) -> [u8; 3] {
        let column = (x * self.scale) as i32;
        let row = (y * self.scale) as i32;
        let at = ((row * self.width + column) * 4) as usize;
        [self.bgra[at + 2], self.bgra[at + 1], self.bgra[at]]
    }

    fn green(&self, x: f32, y: f32) -> bool {
        let [r, g, b] = self.at(x, y);
        g > 200 && r < 70 && b < 70
    }

    fn tinted(&self, x: f32, y: f32) -> bool {
        self.at(x, y)[1] > 40
    }

    fn first_green(&self, y: f32, from: f32, to: f32) -> Option<f32> {
        let mut x = from;
        while x < to {
            if self.tinted(x, y) {
                return Some(x);
            }
            x += 1.0 / self.scale;
        }
        None
    }
}

fn red(pixel: [u8; 3]) -> bool {
    pixel[0] > 200 && pixel[2] < 60
}

fn blue(pixel: [u8; 3]) -> bool {
    pixel[2] > 200 && pixel[0] < 60
}

fn shape(left: f32, top: f32, width: f32, height: f32, transform: Transform2D) -> Element {
    UiBox::new()
        .style(
            Style::new()
                .width(px(width))
                .height(px(height))
                .absolute_at(Some(left), Some(top), None, None)
                .transform(transform),
        )
        .fill(Oklch::from_hex("#00ff00").unwrap())
        .build()
}

fn root(rows: Vec<u32>, ground: bool) -> Element {
    let backdrop = Oklch::from_hex(if ground { "#0000ff" } else { "#ff0000" }).unwrap();
    let half = std::f32::consts::FRAC_1_SQRT_2;
    UiBox::new()
        .style(Style::new().width(px(WIDTH)).height(px(HEIGHT)))
        .child(
            UiBox::new()
                .style(
                    Style::new()
                        .width(px(WIDTH))
                        .height(px(HEIGHT))
                        .absolute_at(Some(0.0), Some(0.0), None, None),
                )
                .fill(backdrop)
                .build(),
        )
        .child(shape(
            20.0,
            120.0,
            40.0,
            40.0,
            Transform2D {
                a: half,
                b: half,
                c: -half,
                d: half,
                tx: 0.0,
                ty: 0.0,
            },
        ))
        .child(shape(
            120.0,
            100.0,
            80.0,
            30.0,
            Transform2D {
                a: 1.0,
                b: 0.0,
                c: 1.0,
                d: 1.0,
                tx: 0.0,
                ty: 0.0,
            },
        ))
        .child(shape(
            260.0,
            100.0,
            80.0,
            30.0,
            Transform2D {
                a: 0.0,
                b: 1.0,
                c: -1.0,
                d: 0.0,
                tx: 30.0,
                ty: 0.0,
            },
        ))
        .child(
            UiBox::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(ROW_W))
                        .height(px(120.0))
                        .absolute_at(Some(ROW_X), Some(ROW_Y), None, None),
                )
                .children(rows.iter().map(|id| {
                    UiBox::new()
                        .style(
                            Style::new()
                                .width(px(ROW_W))
                                .height(px(ROW_H))
                                .flex_shrink(0.0),
                        )
                        .fill(Oklch::from_hex("#00ff00").unwrap())
                        .presence(Presence::new().enter(MOTION).exit(MOTION).slide(SLIDE, 0.0))
                        .build()
                        .with_key(*id)
                }))
                .build(),
        )
        .build()
}

fn distinct(values: &[i32]) -> usize {
    let mut seen = values.to_vec();
    seen.dedup();
    seen.len()
}

enum Phase {
    Warm,
    Entering {
        since: Instant,
        lefts: Vec<i32>,
    },
    Leaving {
        since: Instant,
        lefts: Vec<i32>,
    },
    Sweeping {
        since: Instant,
        started: u64,
        pairs: Vec<([u8; 3], [u8; 3])>,
    },
    Done,
}

struct Swap {
    ui: UiApp,
    rows: WriteSignal<Vec<u32>>,
    ground: WriteSignal<bool>,
    phase: Phase,
    ticks: Mailbox<()>,
    completed: u64,
    first: Option<Instant>,
    frames: u64,
    report: Vec<String>,
    failures: Vec<String>,
    out: String,
}

impl Swap {
    fn fail(&mut self, message: String) {
        self.failures.push(message);
    }

    fn check_shapes(&mut self, shot: &Shot) {
        let inside = [
            (20.0, 148.0),
            (20.0, 135.0),
            (34.0, 148.0),
            (145.0, 115.0),
            (205.0, 115.0),
            (130.0, 104.0),
            (275.0, 140.0),
            (275.0, 175.0),
            (285.0, 104.0),
        ];
        let outside = [
            (60.0, 148.0),
            (50.0, 125.0),
            (20.0, 118.0),
            (125.0, 115.0),
            (225.0, 115.0),
            (121.0, 104.0),
            (275.0, 190.0),
            (295.0, 140.0),
            (255.0, 140.0),
            (330.0, 104.0),
        ];
        for (x, y) in inside {
            if !shot.green(x, y) {
                self.fail(format!(
                    "({x},{y}) should be inside a transformed box: {:?}",
                    shot.at(x, y)
                ));
            }
        }
        for (x, y) in outside {
            if shot.green(x, y) {
                self.fail(format!(
                    "({x},{y}) should be outside a transformed box: {:?}",
                    shot.at(x, y)
                ));
            }
        }
        self.report.push("transforms checked".into());
    }

    fn step(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
        let now = ctx.now();
        let first = *self.first.get_or_insert(now);
        self.frames += 1;
        let hwnd = HWND(id.raw() as *mut _);
        let phase = std::mem::replace(&mut self.phase, Phase::Done);
        self.phase = match phase {
            Phase::Warm => match now.saturating_duration_since(first) >= WARM {
                false => Phase::Warm,
                true => {
                    match Shot::take(hwnd) {
                        Ok(shot) => self.check_shapes(&shot),
                        Err(error) => self.fail(error),
                    }
                    self.rows.set(vec![1, 2, 3, 4]);
                    Phase::Entering {
                        since: now,
                        lefts: Vec::new(),
                    }
                }
            },
            Phase::Entering { since, mut lefts } => {
                let y = ROW_Y + 3.0 * ROW_H + ROW_H / 2.0;
                if let Some(left) = Shot::take(hwnd)
                    .ok()
                    .and_then(|shot| shot.first_green(y, ROW_X - 20.0, WIDTH - 1.0))
                {
                    lefts.push(left as i32);
                }
                match now.saturating_duration_since(since) >= WINDOW {
                    false => Phase::Entering { since, lefts },
                    true => {
                        self.report.push(format!("enter {lefts:?}"));
                        let rest = ROW_X as i32;
                        let moving = lefts.windows(2).all(|pair| pair[1] <= pair[0]);
                        if lefts.last() != Some(&rest) || !moving || distinct(&lefts) < 4 {
                            self.fail(format!(
                                "presence did not slide in across frames: {lefts:?}"
                            ));
                        }
                        self.rows.set(vec![1, 2, 3]);
                        Phase::Leaving {
                            since: now,
                            lefts: Vec::new(),
                        }
                    }
                }
            }
            Phase::Leaving { since, mut lefts } => {
                let y = ROW_Y + 3.0 * ROW_H + ROW_H / 2.0;
                let left = Shot::take(hwnd)
                    .ok()
                    .and_then(|shot| shot.first_green(y, ROW_X - 20.0, WIDTH - 1.0));
                if let Some(left) = left {
                    lefts.push(left as i32);
                }
                match now.saturating_duration_since(since) >= WINDOW {
                    false => Phase::Leaving { since, lefts },
                    true => {
                        self.report.push(format!("leave {lefts:?} final {left:?}"));
                        if left.is_some()
                            || !lefts.windows(2).all(|pair| pair[1] >= pair[0])
                            || distinct(&lefts) < 4
                        {
                            self.fail(format!(
                                "presence did not slide out across frames: {lefts:?}"
                            ));
                        }
                        ctx.request_window(WindowRequest::BeginSnapshotTransition(
                            id,
                            aqua_core::scene::SnapshotTransition {
                                duration: Duration::from_millis(600),
                                band: 0.3,
                                shimmer: Oklch::from_hex("#ffffff").unwrap(),
                            },
                        ))
                        .unwrap();
                        self.ground.set(true);
                        Phase::Sweeping {
                            since: now,
                            started: self.completed,
                            pairs: Vec::new(),
                        }
                    }
                }
            }
            Phase::Sweeping {
                since,
                started,
                mut pairs,
            } => {
                if let Ok(shot) = Shot::take(hwnd) {
                    pairs.push((shot.at(4.0, 4.0), shot.at(WIDTH - 4.0, HEIGHT - 4.0)));
                }
                match now.saturating_duration_since(since) >= Duration::from_millis(1200) {
                    false => Phase::Sweeping {
                        since,
                        started,
                        pairs,
                    },
                    true => {
                        let between = pairs
                            .iter()
                            .filter(|(near, far)| blue(*near) && red(*far))
                            .count();
                        let presented = self.completed - started;
                        self.report.push(format!(
                            "sweep samples {} with near new and far old {between}, {presented} presented frames reported",
                            pairs.len()
                        ));
                        if presented < 10 {
                            self.fail(format!("only {presented} presented frames were reported during a 600 ms transition"));
                        }
                        let first = pairs.first().copied();
                        let last = pairs.last().copied();
                        if between < 2 {
                            self.fail(format!(
                                "no diagonal sweep was presented: {between} of {}",
                                pairs.len()
                            ));
                        }
                        if !first.is_some_and(|(near, far)| red(near) || (blue(near) && red(far))) {
                            self.fail(format!(
                                "transition did not start on the old frame: {first:?}"
                            ));
                        }
                        if !last.is_some_and(|(near, far)| blue(near) && blue(far)) {
                            self.fail(format!("transition did not end on the new frame: {last:?}"));
                        }
                        let line = match self.failures.is_empty() {
                            true => format!("OK {}", self.report.join("; ")),
                            false => format!(
                                "FAILED {} | {}",
                                self.failures.join(" | "),
                                self.report.join("; ")
                            ),
                        };
                        let _ = std::fs::write(&self.out, line);
                        ctx.quit();
                        Phase::Done
                    }
                }
            }
            Phase::Done => Phase::Done,
        };
    }
}

impl Application for Swap {
    fn on_start(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
        self.ui.on_start(ctx, id);
        ctx.bind_mailbox(id, &self.ticks);
        let sender = self.ticks.sender();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(8));
            match sender.send(()) {
                Ok(()) | Err(aqua::mailbox::SendError::Full(())) => {}
                Err(aqua::mailbox::SendError::Closed(())) => break,
            }
        });
    }

    fn on_wake(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
        if !self.ticks.drain().is_empty() {
            self.step(ctx, id);
        }
    }

    fn on_window_event(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
        self.ui.on_window_event(ctx, id, event);
    }

    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &InputEvent) {
        self.ui.on_input(ctx, id, event);
    }

    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
        self.ui.build_scene(window, now)
    }

    fn scene_mut(&mut self, id: AquaWindowId) -> Option<&mut Scene> {
        Application::scene_mut(&mut self.ui, id)
    }

    fn step_input(&mut self, id: AquaWindowId, input: aqua::RuntimeInput) {
        self.ui.step_input(id, input);
    }

    fn take_output(&mut self, id: AquaWindowId) -> aqua::RuntimeOutput {
        self.ui.take_output(id)
    }

    fn reactive_runtime(&self) -> Option<&aqua_reactive::Runtime> {
        self.ui.reactive_runtime()
    }

    fn reactive_runtime_for(&self, id: AquaWindowId) -> Option<&aqua_reactive::Runtime> {
        self.ui.reactive_runtime_for(id)
    }

    fn on_frame_completed(&mut self, ctx: &mut AppCtx, id: AquaWindowId, sequence: u64) {
        self.ui.on_frame_completed(ctx, id, sequence);
        self.completed += 1;
    }
}

fn child(out: String) {
    std::env::set_var("AQUA_RENDER_WHEN_COVERED", "1");
    std::env::set_var("AQUA_SMOKE_MS", "30000");
    let mut config = AquaConfig::default();
    config.window.title = "aqua host swapchain".into();
    config.window.inner_size = LogicalSize::new(f64::from(WIDTH), f64::from(HEIGHT));
    let aqua = Aqua::new(config).expect("aqua");
    let mut ui = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
    let (rows, set_rows) = ui.scope().signal(vec![1u32, 2, 3]);
    let (ground, set_ground) = ui.scope().signal(false);
    ui.mount_reactive((rows, ground), |(rows, ground)| root(rows, ground))
        .expect("mount");
    let app = Swap {
        ui,
        rows: set_rows,
        ground: set_ground,
        phase: Phase::Warm,
        ticks: Mailbox::new(NonZeroUsize::new(8).unwrap()),
        completed: 0,
        first: None,
        frames: 0,
        report: Vec::new(),
        failures: Vec::new(),
        out,
    };
    if let Err(error) = aqua.run(app) {
        eprintln!("host_swapchain: run failed: {error}");
        std::process::exit(1);
    }
}

#[path = "../../aqua-verify/tests/support/desktop.rs"]
mod desktop;

pub fn run() {
    if let Ok(out) = std::env::var("AQUA_HOST_SWAPCHAIN_OUT") {
        child(out);
        return;
    }
    if !desktop::require("host_swapchain") {
        return;
    }
    let out = std::env::temp_dir().join("aqua-host-swapchain.txt");
    let _ = std::fs::remove_file(&out);
    let status = Command::new(std::env::current_exe().unwrap())
        .env("AQUA_HOST_SWAPCHAIN_OUT", &out)
        .status()
        .expect("spawn child");
    let line = std::fs::read_to_string(&out)
        .unwrap_or_else(|e| format!("FAILED no result: {e} ({status})"));
    println!("{line}");
    match line.starts_with("OK") && status.success() {
        true => eprintln!("host_swapchain: ok"),
        false => {
            eprintln!("host_swapchain: FAILED");
            std::process::exit(1);
        }
    }
}

}

fn main() {
    #[cfg(windows)]
    native::run();
    #[cfg(not(windows))]
    eprintln!("host_swapchain: skipped; requires Windows");
}
