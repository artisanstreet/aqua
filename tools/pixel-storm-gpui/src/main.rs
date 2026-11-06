#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::{
    App, AppContext, Bounds, Context, Div, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, rgb, size,
};
use std::time::{Duration, Instant};

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
        (x.wrapping_mul(y).wrapping_pow(2).wrapping_add(t.wrapping_mul(7)) & 0xff) as u8,
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

fn hex([r, g, b]: [u8; 3]) -> u32 {
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

fn grid_colors(grid: Grid, t: u32) -> Vec<u32> {
    (0..grid.height)
        .flat_map(|y| row_colors(y, grid.width, t).map(hex))
        .collect()
}

fn cell(color: u32) -> Div {
    div().flex_none().w(px(1.0)).h(px(1.0)).bg(rgb(color))
}

fn row(colors: &[u32], width: u32) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .w(px(width as f32))
        .h(px(1.0))
        .children(colors.iter().copied().map(cell))
}

fn storm(colors: &[u32], grid: Grid) -> Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .children(
            colors
                .chunks(grid.width as usize)
                .map(|line| row(line, grid.width)),
        )
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
    origin: Instant,
    last: Option<Instant>,
    window_start: Instant,
    window_index: u32,
    samples: Vec<f64>,
}

impl Meter {
    fn new(origin: Instant) -> Self {
        Self {
            origin,
            last: None,
            window_start: origin,
            window_index: 0,
            samples: Vec::new(),
        }
    }

    fn presented(&mut self, now: Instant, settings: Settings) {
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
    settings: Settings,
    frame: u32,
    colors: Vec<u32>,
    meter: Meter,
}

impl Storm {
    fn presented(&mut self, cx: &mut Context<Self>) {
        self.meter.presented(Instant::now(), self.settings);
        if self.settings.mode == Mode::Animated {
            self.frame = self.frame.wrapping_add(1);
        }
        cx.notify();
    }
}

impl Render for Storm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = Instant::now();
        if self.settings.mode == Mode::Animated {
            self.colors = grid_colors(self.settings.grid, self.frame);
        }
        let element = storm(&self.colors, self.settings.grid);
        eprintln!(
            "pixel-storm render t={} build_ms={:.1}",
            self.frame,
            started.elapsed().as_secs_f64() * 1000.0
        );
        let entity = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            entity
                .update(cx, |storm, cx| storm.presented(cx))
                .ok();
        });
        element
    }
}

fn main() {
    let origin = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let settings = parse_settings(&args);
    let grid = settings.grid;
    gpui_platform::application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(
            None,
            size(px(grid.width as f32), px(grid.height as f32)),
            cx,
        );
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            focus: false,
            is_resizable: false,
            inactive_frame_interval: None,
            ..WindowOptions::default()
        };
        let opened = cx.open_window(options, move |_, cx| {
            cx.new(|_| Storm {
                settings,
                frame: 0,
                colors: grid_colors(grid, 0),
                meter: Meter::new(origin),
            })
        });
        if opened.is_err() {
            cx.quit();
        }
    });
}
