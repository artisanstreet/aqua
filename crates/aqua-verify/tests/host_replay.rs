#![cfg(windows)]

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aqua::render::host::RenderStats;
use aqua::render::lower::Lowering;
use aqua::UiApp;
use aqua_core::{DisplayItem, Size};
use aqua_protocol::{
    first_divergence, replay, Instant, Machine, Recorded, RenderCommand, RenderInput, RenderWake,
    RuntimeInput, RuntimeOutput, ShellEvent, Timeline,
};
use aqua_runtime::{RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_verify::fingerprint;

#[path = "support/host.rs"]
mod host;
#[path = "support/ui.rs"]
mod ui;

use host::{open, scripted_session, Captured, Host, Kind, HEIGHT, KINDS, WIDTH};
use ui::editor_like;

type Log = Vec<Recorded<RuntimeInput, RuntimeOutput>>;

fn fresh_runtime(fonts: SharedFonts) -> Runtime {
    fresh_runtime_with(fonts, SharedRuns::new())
}

fn fresh_runtime_with(fonts: SharedFonts, runs: SharedRuns) -> Runtime {
    let mut app = UiApp::new(fonts, runs);
    editor_like(&mut app, WIDTH as f32, HEIGHT as f32);
    app.into_runtime()
}

fn scene_hash(runtime: &Runtime) -> u64 {
    let items: Vec<DisplayItem> = runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    fingerprint::of(&items)
}

fn recorded(kind: Kind) -> (Host, Log) {
    let mut host = open(kind).unwrap_or_else(|| panic!("{kind:?} must present"));
    scripted_session(&mut host);
    let log = host.app.recorded_session(host.id).to_vec();
    (host, log)
}

fn stamp(input: &RuntimeInput) -> Option<Instant> {
    match input {
        RuntimeInput::Tick(at) => Some(*at),
        RuntimeInput::Shell(ShellEvent::Frame { presentation, .. }) => Some(*presentation),
        _ => None,
    }
}

fn offline_final_frame(kind: Kind, fonts: &SharedFonts, log: &Log) -> Option<u64> {
    let runs = SharedRuns::new();
    let mut runtime = fresh_runtime_with(fonts.clone(), runs.clone());
    let (_updates, externals) = channel();
    let captured = Arc::new(Mutex::new(Captured::default()));
    let mut target = host::target(
        kind,
        Lowering::new(fonts.clone(), runs),
        externals,
        Arc::new(RenderStats::default()),
        captured.clone(),
    )
    .ok()?;
    let mut side = RenderSide::new();
    let mut present_at = Instant::ZERO;
    let mut step = |side: &mut RenderSide, commands: Vec<RenderCommand>, at: Instant| {
        let output = side.step(RenderInput {
            commands,
            feedback: Vec::new(),
            present_at: at,
        });
        if let Some(frame) = output.frame {
            target.wait_frame();
            target.present(side, frame);
        }
        output.wake
    };
    step(
        &mut side,
        vec![RenderCommand::Viewport {
            size: Size {
                w: WIDTH as f32,
                h: HEIGHT as f32,
            },
            scale: 1.0,
        }],
        present_at,
    );
    for entry in log {
        present_at = stamp(&entry.input).unwrap_or(present_at);
        let output = runtime.step(entry.input.clone());
        if !output.render.is_empty() {
            step(&mut side, output.render, present_at);
        }
    }
    for _ in 0..600 {
        present_at += Duration::from_millis(16);
        if step(&mut side, Vec::new(), present_at) == RenderWake::Idle {
            break;
        }
    }
    let hash = captured.lock().unwrap().hashes.last().copied();
    hash
}

#[test]
fn a_live_host_session_replays_to_identical_runtime_outputs_and_scene() {
    let (host, log) = recorded(Kind::Reference);
    assert!(
        log.len() > 50,
        "the session recorded only {} steps",
        log.len()
    );
    assert!(log.iter().any(|e| !e.output.render.is_empty()));
    let mut fresh = fresh_runtime(host.aqua.fonts().clone());
    let replayed = replay(&mut fresh, log.iter().map(|entry| entry.input.clone()));
    assert_eq!(first_divergence(&log, &replayed), None);
    assert_eq!(scene_hash(&fresh), scene_hash(&host.app));
}

#[test]
fn replayed_render_commands_reproduce_the_live_final_frame_on_every_backend() {
    for kind in KINDS {
        let (host, log) = recorded(kind);
        let live = host.last_hash().expect("live frame");
        let offline = offline_final_frame(kind, host.aqua.fonts(), &log).expect("offline frame");
        assert_eq!(
            live, offline,
            "{kind:?}: replay produced a different final frame"
        );
    }
}

#[test]
fn two_live_sessions_replay_to_the_same_final_scene_and_frame() {
    let (first_host, first) = recorded(Kind::Reference);
    let (second_host, second) = recorded(Kind::Reference);
    assert_eq!(scene_hash(&first_host.app), scene_hash(&second_host.app));
    assert_eq!(first_host.last_hash(), second_host.last_hash());
    assert_eq!(
        offline_final_frame(Kind::Reference, first_host.aqua.fonts(), &first),
        offline_final_frame(Kind::Reference, second_host.aqua.fonts(), &second)
    );
}

fn timeline_over(
    fonts: &SharedFonts,
    inputs: impl Iterator<Item = RuntimeInput>,
) -> Timeline<Runtime> {
    let fonts = fonts.clone();
    let mut line = Timeline::new(move || fresh_runtime(fonts.clone()));
    inputs.for_each(|input| {
        line.step(input);
    });
    line
}

#[test]
fn seeking_a_recorded_host_session_lands_on_the_same_scene_as_replaying_its_prefix() {
    let (host, log) = recorded(Kind::Reference);
    let fonts = host.aqua.fonts().clone();
    let inputs: Vec<RuntimeInput> = log.iter().map(|entry| entry.input.clone()).collect();
    let mut line = timeline_over(&fonts, inputs.iter().cloned());
    let end = line.now();
    let end_hash = scene_hash(line.machine());
    assert!(end > Instant::ZERO);

    let middle = Instant::from_nanos(end.as_nanos() / 2);
    line.seek(middle);
    let landed = line.cursor();
    assert!(landed > 0 && landed < inputs.len());
    let middle_hash = scene_hash(line.machine());
    assert_ne!(middle_hash, end_hash, "the session never changed the scene");

    let prefix = timeline_over(&fonts, inputs.iter().take(landed).cloned());
    assert_eq!(scene_hash(prefix.machine()), middle_hash);

    line.seek(Instant::ZERO);
    assert_ne!(scene_hash(line.machine()), end_hash);
    line.seek(end);
    assert_eq!(line.cursor(), inputs.len());
    assert_eq!(scene_hash(line.machine()), end_hash);
}

#[test]
fn a_paused_timeline_over_the_host_runtime_freezes_until_stepped() {
    let (host, log) = recorded(Kind::Reference);
    let fonts = host.aqua.fonts().clone();
    let inputs: Vec<RuntimeInput> = log.iter().map(|entry| entry.input.clone()).collect();
    let split = inputs.len() / 2;
    let mut line = timeline_over(&fonts, inputs.iter().take(split).cloned());
    line.pause();
    let frozen = line.now();
    inputs
        .iter()
        .skip(split)
        .filter(|input| stamp(input).is_some())
        .for_each(|input| {
            line.step(input.clone());
        });
    assert_eq!(line.now(), frozen);
    line.advance(
        Duration::from_millis(16),
        &[aqua_protocol::WindowId::new(1).unwrap()],
    );
    assert_eq!(line.now(), frozen + Duration::from_millis(16));
}

#[test]
fn slow_motion_and_stepping_a_host_session_are_deterministic_and_scale_time() {
    let (host, log) = recorded(Kind::Reference);
    let fonts = host.aqua.fonts().clone();
    let inputs: Vec<RuntimeInput> = log.iter().map(|entry| entry.input.clone()).collect();
    let run = |rate: f64| {
        let mut line = Timeline::new({
            let fonts = fonts.clone();
            move || fresh_runtime(fonts.clone())
        });
        line.slow(rate);
        inputs.iter().for_each(|input| {
            line.step(input.clone());
        });
        line
    };
    let full = run(1.0);
    let half_a = run(0.5);
    let half_b = run(0.5);
    assert_eq!(scene_hash(half_a.machine()), scene_hash(half_b.machine()));
    assert_eq!(half_a.now(), half_b.now());
    let full_ns = full.now().as_nanos() as f64;
    let half_ns = half_a.now().as_nanos() as f64;
    assert!(half_ns < full_ns * 0.75 && half_ns > full_ns * 0.25);

    let window = [aqua_protocol::WindowId::new(1).unwrap()];
    let step_frames = || {
        let mut line = timeline_over(&fonts, inputs.iter().cloned());
        line.pause();
        (0..30).for_each(|_| {
            line.advance(Duration::from_millis(16), &window);
        });
        line
    };
    let first = step_frames();
    let second = step_frames();
    assert_eq!(scene_hash(first.machine()), scene_hash(second.machine()));
    assert_eq!(first.cursor(), second.cursor());
}
