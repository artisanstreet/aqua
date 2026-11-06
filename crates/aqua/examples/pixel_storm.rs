#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use aqua::prelude::*;
use aqua::{pct, px, Aqua, Style};
use aqua_protocol::LogicalSize;
use aqua_reactive::WriteSignal;
use aqua_ui::element::Element;
use std::time::{Duration, Instant as WallInstant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Animated,
    Static,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Grid {
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Settings {
    grid: Grid,
    mode: Mode,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Summary {
    frames: usize,
    p50: f64,
    p99: f64,
    max: f64,
}

const WINDOW: Duration = Duration::from_secs(10);

const PAGE: Style = Aqua::Style().width(pct(100.0)).height(pct(100.0)).column();
const ROW: Style = Aqua::Style().row().height(px(1.0)).flex_shrink(0.0);
const CELL: Style = Aqua::Style()
    .width(px(1.0))
    .height(px(1.0))
    .flex_shrink(0.0);

fn parse_grid(text: &str) -> Option<Grid> {
    let (w, h) = text.split_once('x')?;
    Some(Grid {
        width: w.parse().ok()?,
        height: h.parse().ok()?,
    })
}

fn parse_settings(args: &[String]) -> Settings {
    let grid = args
        .iter()
        .position(|arg| arg == "--size")
        .and_then(|index| args.get(index + 1))
        .and_then(|text| parse_grid(text))
        .unwrap_or(Grid {
            width: 1280,
            height: 800,
        });
    let mode = match args.iter().any(|arg| arg == "--static") {
        true => Mode::Static,
        false => Mode::Animated,
    };
    Settings { grid, mode }
}

fn base(x: u32, y: u32, t: u32) -> [u8; 3] {
    [
        (x.wrapping_pow(2).wrapping_add(t.wrapping_mul(3)) & 0xff) as u8,
        (y.wrapping_pow(3).wrapping_add(t.wrapping_mul(5)) & 0xff) as u8,
        (x.wrapping_mul(y)
            .wrapping_pow(2)
            .wrapping_add(t.wrapping_mul(7))
            & 0xff) as u8,
    ]
}

fn fold(previous: [u8; 3], cell: [u8; 3]) -> [u8; 3] {
    [
        ((u16::from(cell[0]) + u16::from(previous[0])) / 2) as u8,
        cell[1],
        cell[2] ^ (previous[0] >> 3),
    ]
}

fn row_colors(y: u32, width: u32, t: u32) -> impl Iterator<Item = [u8; 3]> {
    (0..width).scan([0u8; 3], move |previous, x| {
        *previous = fold(*previous, base(x, y, t));
        Some(*previous)
    })
}

fn cell(color: [u8; 3]) -> Element {
    Aqua::Box()
        .style(CELL.background(Oklch::from_srgb8(color[0], color[1], color[2], 1.0)))
        .build()
}

fn row(y: u32, grid: Grid, t: u32) -> Element {
    Aqua::Box()
        .style(ROW.width(px(grid.width as f32)))
        .children(row_colors(y, grid.width, t).map(cell))
        .build()
}

fn storm(grid: Grid, t: u32) -> Element {
    Aqua::Box()
        .styles([PAGE])
        .children((0..grid.height).map(|y| row(y, grid, t)))
        .build()
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn summarize(samples: &[f64]) -> Option<Summary> {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    (!sorted.is_empty()).then(|| Summary {
        frames: sorted.len(),
        p50: percentile(&sorted, 0.5),
        p99: percentile(&sorted, 0.99),
        max: sorted[sorted.len() - 1],
    })
}

struct Meter {
    origin: WallInstant,
    last: Option<WallInstant>,
    window_start: WallInstant,
    window_index: u32,
    samples: Vec<f64>,
}

impl Meter {
    fn new(origin: WallInstant) -> Self {
        Self {
            origin,
            last: None,
            window_start: origin,
            window_index: 0,
            samples: Vec::new(),
        }
    }

    fn presented(&mut self, now: WallInstant, settings: Settings) {
        match self.last {
            None => {
                eprintln!(
                    "pixel-storm first_frame_ms={:.1} boxes={} mode={:?} grid={}x{}",
                    (now - self.origin).as_secs_f64() * 1000.0,
                    settings.grid.width * settings.grid.height,
                    settings.mode,
                    settings.grid.width,
                    settings.grid.height
                );
                self.window_start = now;
            }
            Some(last) => self.samples.push((now - last).as_secs_f64() * 1000.0),
        }
        self.last = Some(now);
        if now - self.window_start >= WINDOW {
            if let Some(summary) = summarize(&self.samples) {
                eprintln!(
                    "pixel-storm window={} seconds={:.1} frames={} p50_ms={:.2} p99_ms={:.2} max_ms={:.2}",
                    self.window_index,
                    (now - self.window_start).as_secs_f64(),
                    summary.frames,
                    summary.p50,
                    summary.p99,
                    summary.max
                );
            }
            self.window_index += 1;
            self.window_start = now;
            self.samples.clear();
        }
    }
}

struct Storm {
    ui: UiApp,
    settings: Settings,
    frame: WriteSignal<u32>,
    meter: Meter,
}

impl Application for Storm {
    fn input_invalidation_epoch(&self, id: AquaWindowId) -> u64 {
        Application::input_invalidation_epoch(&self.ui, id)
    }

    fn color_profile(&self) -> aqua_core::ColorProfile {
        Application::color_profile(&self.ui)
    }

    fn color_profile_for(&self, id: AquaWindowId) -> aqua_core::ColorProfile {
        Application::color_profile_for(&self.ui, id)
    }

    fn on_click_settings(&mut self, settings: aqua_core::ClickSettings) {
        Application::on_click_settings(&mut self.ui, settings);
    }

    fn on_start(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
        Application::on_start(&mut self.ui, ctx, id);
    }

    fn on_frame_completed(&mut self, ctx: &mut AppCtx, id: AquaWindowId, _sequence: u64) {
        self.meter.presented(WallInstant::now(), self.settings);
        match self.settings.mode {
            Mode::Animated => self.frame.update(|t| *t = t.wrapping_add(1)),
            Mode::Static => ctx.request_frame(id, FrameReason::EXPLICIT),
        }
    }

    fn cursor_for(&self, id: AquaWindowId) -> Option<CursorIcon> {
        Application::cursor_for(&self.ui, id)
    }

    fn a11y_nodes_for(&self, id: AquaWindowId) -> Option<(NodeId, NodeId, Vec<A11yNode>)> {
        Application::a11y_nodes_for(&self.ui, id)
    }

    fn step_input(&mut self, id: AquaWindowId, input: RuntimeInput) {
        Application::step_input(&mut self.ui, id, input);
    }

    fn take_output(&mut self, id: AquaWindowId) -> RuntimeOutput {
        Application::take_output(&mut self.ui, id)
    }

    fn on_window_event(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
        Application::on_window_event(&mut self.ui, ctx, id, event);
    }

    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &InputEvent) {
        Application::on_input(&mut self.ui, ctx, id, event);
    }

    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
        Application::build_scene(&mut self.ui, window, now)
    }

    fn reactive_runtime(&self) -> Option<&ReactiveRuntime> {
        Application::reactive_runtime(&self.ui)
    }

    fn reactive_runtime_for(&self, id: AquaWindowId) -> Option<&ReactiveRuntime> {
        Application::reactive_runtime_for(&self.ui, id)
    }
}

fn main() -> Result<(), aqua::Error> {
    let origin = WallInstant::now();
    let args: Vec<String> = std::env::args().collect();
    let settings = parse_settings(&args);
    let grid = settings.grid;
    let config = AquaConfig {
        window: WindowOptions {
            title: "Aqua pixel storm".to_string(),
            inner_size: LogicalSize::new(f64::from(grid.width), f64::from(grid.height)),
            resizable: false,
            ..WindowOptions::default()
        },
        ..AquaConfig::default()
    };
    let aqua = Aqua::new(config).map_err(aqua::Error::Init)?;
    let mut ui = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
    ui.set_frame_stats(std::env::var_os("AQUA_FRAME_STATS").is_some_and(|value| value == "1"));
    let (frame_read, frame_write) = ui.scope().signal(0u32);
    ui.mount_reactive((frame_read,), move |(t,)| {
        let started = WallInstant::now();
        let element = storm(grid, t);
        eprintln!(
            "pixel-storm render t={t} build_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0
        );
        element
    })
    .map_err(aqua::Error::Mount)?;
    aqua.run(Storm {
        ui,
        settings,
        frame: frame_write,
        meter: Meter::new(origin),
    })
    .map_err(aqua::Error::Runtime)
}
