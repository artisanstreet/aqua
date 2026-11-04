#![cfg(windows)]

use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aqua::config::AquaConfig;
use aqua::render::host::RenderStats;
use aqua::runtime::{AppCtx, Application, Aqua};
use aqua_core::{DisplayItem, InputEvent, NodeId, Oklch, Rect};
use aqua_protocol::geometry::AquaWindowId;
use aqua_protocol::window::{WindowOptions, WindowRequest};
use aqua_protocol::{Instant, ObservedWindowState, WindowEvent};
use aqua_scene::Scene;

const WARM_FRAMES: u64 = 90;
const WINDOWS: usize = 3;

fn block(x: f32, y: f32, color: &str) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x,
            y,
            w: 24.0,
            h: 24.0,
        },
        radius: 4.0,
        fill: Oklch::from_hex(color).unwrap(),
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    }
}

struct Canvas {
    scene: Scene,
    mover: NodeId,
    step: f32,
}

impl Canvas {
    fn new() -> Self {
        let mut scene = Scene::new();
        scene.insert(DisplayItem::Rect {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: 2000.0,
                h: 2000.0,
            },
            radius: 0.0,
            fill: Oklch::from_hex("#101820").unwrap(),
            border_width: 0.0,
            border_color: Oklch::TRANSPARENT,
            shadow: None,
        });
        for row in 0..8 {
            for column in 0..10 {
                scene.insert(block(
                    20.0 + column as f32 * 40.0,
                    60.0 + row as f32 * 30.0,
                    "#456789",
                ));
            }
        }
        let mover = scene.insert(block(10.0, 10.0, "#ffaa33"));
        Self {
            scene,
            mover,
            step: 0.0,
        }
    }

    fn advance(&mut self) {
        self.step += 3.0;
        self.scene
            .update(self.mover, block(10.0 + self.step % 380.0, 10.0, "#ffaa33"));
    }
}

#[derive(Default)]
struct Outcome {
    verdict: Option<Result<String, String>>,
}

struct Probe {
    canvases: HashMap<AquaWindowId, Canvas>,
    started: Vec<AquaWindowId>,
    completed: HashMap<AquaWindowId, u64>,
    phase: Phase,
    outcome: Arc<Mutex<Outcome>>,
}

struct Mark {
    frames: u64,
    attached: u64,
}

enum Phase {
    Warm,
    Trimming(Vec<(AquaWindowId, u64)>),
    Recovering(Vec<(AquaWindowId, Mark)>),
    Done,
}

impl Probe {
    fn stats_of(&self, ctx: &AppCtx) -> Vec<(AquaWindowId, Arc<RenderStats>)> {
        self.started
            .iter()
            .filter_map(|id| ctx.render_stats(*id).map(|stats| (*id, stats)))
            .collect()
    }

    fn finish(&mut self, ctx: &mut AppCtx, verdict: Result<String, String>) {
        self.outcome.lock().unwrap().verdict = Some(verdict);
        self.phase = Phase::Done;
        ctx.quit();
    }

    fn warm_verdict(&self, stats: &[(AquaWindowId, Arc<RenderStats>)]) -> Result<String, String> {
        let threads: Vec<u64> = stats
            .iter()
            .map(|(_, s)| s.service_thread.load(Relaxed))
            .collect();
        if threads.iter().any(|t| *t == 0 || *t != threads[0]) {
            return Err(format!(
                "windows do not share one render thread: {threads:?}"
            ));
        }
        let sharers = stats
            .iter()
            .map(|(_, s)| s.device_sharers.load(Relaxed))
            .max()
            .unwrap_or(0);
        if sharers < WINDOWS as u64 {
            return Err(format!(
                "device is shared by {sharers} windows, wanted {WINDOWS}"
            ));
        }
        let mut lines = Vec::new();
        for (id, s) in stats {
            let partial = s.partial_frames.load(Relaxed);
            let full = s.full_frames.load(Relaxed);
            let pixels = s.damaged_pixels.load(Relaxed);
            lines.push(format!(
                "window {}: frames={} partial={partial} full={full} damaged_px_per_partial={}",
                id.raw(),
                s.frames.load(Relaxed),
                pixels / partial.max(1)
            ));
            if s.failures.load(Relaxed) != 0 {
                return Err(format!("window {} reported render failures", id.raw()));
            }
            if partial < full * 3 {
                return Err(format!(
                    "window {} was mostly repainted in full (partial {partial}, full {full})",
                    id.raw()
                ));
            }
            if pixels / partial.max(1) > 200 * 200 {
                return Err(format!("window {} partial damage is too large", id.raw()));
            }
        }
        let attached: u64 = stats.iter().map(|(_, s)| s.attached.load(Relaxed)).sum();
        if attached == 0 {
            return Err("the transparent window never attached its composition surface".into());
        }
        lines.push(format!("composition surfaces attached: {attached}"));
        Ok(lines.join("\n"))
    }
}

impl Application for Probe {
    fn on_start(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
        self.started.push(id);
        if self.started.len() == 1 {
            for (title, transparent) in [("aqua shared b", false), ("aqua shared c", true)] {
                ctx.request_window(WindowRequest::Create(WindowOptions {
                    title: title.into(),
                    transparent,
                    ..WindowOptions::default()
                }))
                .unwrap();
            }
        }
    }

    fn on_window_event(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &WindowEvent) {}

    fn on_input(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &InputEvent) {}

    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
        let canvas = self.canvases.entry(window.id).or_insert_with(Canvas::new);
        canvas.advance();
        Some(now + Duration::from_millis(8))
    }

    fn scene_mut(&mut self, id: AquaWindowId) -> Option<&mut Scene> {
        self.canvases.get_mut(&id).map(|canvas| &mut canvas.scene)
    }

    fn on_frame_completed(&mut self, ctx: &mut AppCtx, id: AquaWindowId, _sequence: u64) {
        *self.completed.entry(id).or_default() += 1;
        let stats = self.stats_of(ctx);
        let warm = self.started.len() == WINDOWS
            && stats.len() == WINDOWS
            && self.completed.len() == WINDOWS
            && self.completed.values().all(|n| *n >= WARM_FRAMES);
        match &mut self.phase {
            Phase::Warm if warm => match self.warm_verdict(&stats) {
                Ok(_) => {
                    let marks = stats
                        .iter()
                        .map(|(id, s)| (*id, s.trims.load(Relaxed)))
                        .collect();
                    stats.iter().for_each(|(id, _)| ctx.request_trim(*id));
                    self.phase = Phase::Trimming(marks);
                }
                Err(message) => self.finish(ctx, Err(message)),
            },
            Phase::Trimming(marks) => {
                let trimmed = marks.iter().all(|(id, before)| {
                    stats
                        .iter()
                        .find(|(other, _)| other == id)
                        .is_some_and(|(_, s)| s.trims.load(Relaxed) > *before)
                });
                if trimmed {
                    match self.warm_verdict(&stats) {
                        Ok(lines) => {
                            println!("{lines}\ntrim requests executed on every window");
                            let marks = stats
                                .iter()
                                .map(|(id, s)| {
                                    (
                                        *id,
                                        Mark {
                                            frames: s.frames.load(Relaxed),
                                            attached: s.attached.load(Relaxed),
                                        },
                                    )
                                })
                                .collect();
                            ctx.inject_device_loss(self.started[0]);
                            self.phase = Phase::Recovering(marks);
                        }
                        Err(message) => self.finish(ctx, Err(message)),
                    }
                }
            }
            Phase::Recovering(marks) => {
                let moving = marks.iter().all(|(id, mark)| {
                    stats
                        .iter()
                        .find(|(other, _)| other == id)
                        .is_some_and(|(_, s)| s.frames.load(Relaxed) > mark.frames + 20)
                });
                let recoveries: u64 = stats.iter().map(|(_, s)| s.recoveries.load(Relaxed)).sum();
                let attached_now: u64 = stats.iter().map(|(_, s)| s.attached.load(Relaxed)).sum();
                let attached_before: u64 = marks.iter().map(|(_, m)| m.attached).sum();
                if moving && recoveries > 0 {
                    let verdict = match attached_now > attached_before {
                        true => Ok(format!(
                            "device loss recovered on {recoveries} windows, composition surface re-attached"
                        )),
                        false => Err("composition surface was not re-attached after recovery".into()),
                    };
                    self.finish(ctx, verdict);
                }
            }
            _ => {}
        }
    }
}

fn main() {
    std::env::set_var("AQUA_SMOKE_MS", "40000");
    std::env::set_var("AQUA_RENDER_WHEN_COVERED", "1");
    let outcome = Arc::new(Mutex::new(Outcome::default()));
    let mut config = AquaConfig::default();
    config.window.title = "aqua shared a".into();
    let aqua = Aqua::new(config).expect("aqua");
    let probe = Probe {
        canvases: HashMap::new(),
        started: Vec::new(),
        completed: HashMap::new(),
        phase: Phase::Warm,
        outcome: outcome.clone(),
    };
    if let Err(error) = aqua.run(probe) {
        eprintln!("render_shared: run failed: {error}");
        std::process::exit(1);
    }
    let verdict = outcome.lock().unwrap().verdict.take();
    match verdict {
        Some(Ok(report)) => eprintln!("{report}\nrender_shared: ok"),
        Some(Err(message)) => {
            eprintln!("render_shared: FAILED: {message}");
            std::process::exit(1);
        }
        None => {
            eprintln!("render_shared: FAILED: the probe never reached a verdict");
            std::process::exit(1);
        }
    }
}
