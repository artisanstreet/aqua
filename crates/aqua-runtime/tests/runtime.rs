use aqua_core::{
    DisplayItem, InputEvent, Key, KeyEvent, Modifiers, Point, PointerButton, PointerEvent,
    PointerKind,
};
use aqua_protocol::{
    first_divergence, replay, FrameReason, ImeCommand, ImeEvent, Instant, Machine, PixelSize,
    Recorder, RenderCommand, RequestId, RuntimeInput, RuntimeOutput, ShellCommand, ShellEvent,
    Utf16Range,
};
use aqua_protocol::{RenderInput, RenderWake};
use aqua_runtime::{RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::motion::{Easing, MotionKind, MotionValue};
use aqua_ui::style::{px, Style};

mod support;

fn mounted() -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiBox::new()
                .child(
                    UiText::builder()
                        .arg("draft")
                        .editable(true)
                        .style(Style::new().width(px(200.0)).height(px(32.0)))
                        .build(),
                )
                .build()
        })
        .unwrap();
    runtime
}

fn pointer(at: Point, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: at,
        buttons,
    }
}

fn press(at: Point) -> InputEvent {
    InputEvent::PointerDown(pointer(at, 1), PointerButton::Primary)
}

fn moved(at: Point) -> InputEvent {
    InputEvent::PointerMove(pointer(at, 0))
}

fn shell_input(event: InputEvent) -> RuntimeInput {
    RuntimeInput::Shell(ShellEvent::Input {
        window: support::window(),
        event,
    })
}

fn frame_input(secs: f64) -> RuntimeInput {
    RuntimeInput::Shell(ShellEvent::Frame {
        window: support::window(),
        presentation: Instant::from_secs_f64(secs),
    })
}

fn resized_input() -> RuntimeInput {
    RuntimeInput::Shell(ShellEvent::Resized {
        window: support::window(),
        size: PixelSize {
            width: 400,
            height: 300,
        },
        scale: 1.0,
    })
}

fn key(key: Key, modifiers: Modifiers) -> InputEvent {
    InputEvent::KeyDown(KeyEvent {
        key,
        modifiers,
        repeat: false,
    })
}

fn script() -> Vec<RuntimeInput> {
    vec![
        resized_input(),
        frame_input(0.0),
        RuntimeInput::Tick(Instant::from_secs_f64(0.1)),
        shell_input(press(Point::new(10.0, 10.0))),
        frame_input(0.1),
        shell_input(InputEvent::TextInput {
            text: "!".to_string(),
        }),
        RuntimeInput::AccessClient { connected: true },
        frame_input(0.116),
        shell_input(key(Key::Char('a'), Modifiers::CONTROL)),
        shell_input(key(Key::Char('c'), Modifiers::CONTROL)),
        shell_input(key(Key::Char('v'), Modifiers::CONTROL)),
        RuntimeInput::Shell(ShellEvent::Clipboard {
            request: RequestId(1),
            text: Some("pasted".to_string()),
            image: None,
            paths: Vec::new(),
        }),
        RuntimeInput::Ime(ImeEvent::Replace {
            range: Utf16Range::new(0, 2),
            text: "xy".to_string(),
        }),
        frame_input(0.2),
        frame_input(0.3),
        shell_input(InputEvent::Focused(false)),
        frame_input(0.4),
    ]
}

fn scene_items(runtime: &Runtime) -> Vec<DisplayItem> {
    runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect()
}

fn tween(secs: f64) -> MotionKind {
    MotionKind::Tween {
        duration_secs: secs,
        easing: Easing::Linear,
    }
}

fn requested(output: &RuntimeOutput) -> FrameReason {
    output
        .shell
        .iter()
        .filter_map(|command| match command {
            ShellCommand::RequestFrame { reason, .. } => Some(*reason),
            _ => None,
        })
        .fold(FrameReason::empty(), |all, reason| all | reason)
}

fn scene_upserts(output: &RuntimeOutput) -> usize {
    output
        .render
        .iter()
        .map(|command| match command {
            RenderCommand::Scene(diff) => diff.upserts.len(),
            _ => 0,
        })
        .sum()
}

#[test]
fn replaying_a_recorded_log_reproduces_every_output_and_the_scene() {
    let mut recorder = Recorder::new(mounted());
    script().into_iter().for_each(|input| {
        recorder.step(input);
    });
    let log = recorder.log().to_vec();
    let mut fresh = mounted();
    let replayed = replay(&mut fresh, log.iter().map(|entry| entry.input.clone()));
    assert_eq!(first_divergence(&log, &replayed), None);
    assert_eq!(scene_items(recorder.machine()), scene_items(&fresh));
    assert!(!fresh.scene().is_empty());
    assert!(log.iter().any(|entry| entry.output.access.is_some()));
    assert!(log.iter().any(|entry| !entry.output.ime.is_empty()));
    assert!(log.iter().any(|entry| scene_upserts(&entry.output) > 0));
}

#[test]
fn replay_reports_the_first_step_whose_input_was_altered() {
    let mut recorder = Recorder::new(mounted());
    script().into_iter().for_each(|input| {
        recorder.step(input);
    });
    let log = recorder.log().to_vec();
    let mut altered: Vec<_> = log.iter().map(|entry| entry.input.clone()).collect();
    altered[5] = shell_input(InputEvent::TextInput {
        text: "?????".to_string(),
    });
    let mut fresh = mounted();
    let replayed = replay(&mut fresh, altered);
    let divergence = first_divergence(&log, &replayed).unwrap();
    assert!((5..=7).contains(&divergence));
}

#[test]
fn first_frame_sends_viewport_then_full_scene_and_idle_frames_send_nothing() {
    let mut runtime = mounted();
    runtime.step(resized_input());
    let first = runtime.step(frame_input(0.0));
    assert!(matches!(
        first.render.first(),
        Some(RenderCommand::Viewport { scale, .. }) if *scale == 1.0
    ));
    let RenderCommand::Scene(diff) = &first.render[1] else {
        panic!("scene diff expected");
    };
    assert_eq!(diff.upserts.len(), runtime.scene().len());
    assert_eq!(
        diff.order.as_ref().map(Vec::len),
        Some(runtime.scene().len())
    );
    let idle = runtime.step(frame_input(0.01));
    assert!(idle.render.is_empty());
    assert!(idle.access.is_none());
}

#[test]
fn emitted_diffs_keep_a_render_side_scene_identical_across_edits() {
    let mut runtime = mounted();
    let mut render = RenderSide::new();
    let mut secs = 0.0;
    let mut deliver = |runtime: &mut Runtime, input: RuntimeInput| {
        let output = runtime.step(input);
        secs += 0.01;
        let stepped = render.step(RenderInput {
            commands: output.render,
            feedback: Vec::new(),
            present_at: Instant::from_secs_f64(secs),
        });
        assert_eq!(stepped.wake, RenderWake::Idle);
        let expected: Vec<_> = runtime
            .scene()
            .ordered()
            .map(|(_, item)| item.clone())
            .collect();
        let presented: Vec<_> = runtime
            .scene()
            .ordered()
            .map(|(id, _)| render.presented(id).cloned())
            .collect();
        assert_eq!(presented.len(), expected.len());
        assert!(presented
            .iter()
            .zip(&expected)
            .all(|(presented, expected)| presented.as_ref() == Some(expected)));
    };
    deliver(&mut runtime, resized_input());
    deliver(&mut runtime, frame_input(0.0));
    deliver(&mut runtime, shell_input(press(Point::new(190.0, 10.0))));
    deliver(&mut runtime, frame_input(0.1));
    for (index, text) in ["a", "b", "c"].into_iter().enumerate() {
        deliver(
            &mut runtime,
            shell_input(InputEvent::TextInput {
                text: text.to_string(),
            }),
        );
        deliver(&mut runtime, frame_input(0.2 + index as f64 * 0.1));
    }
}

#[test]
fn a_content_change_sends_only_changed_items_without_reordering() {
    let mut runtime = mounted();
    runtime.step(resized_input());
    runtime.step(frame_input(0.0));
    runtime.step(shell_input(press(Point::new(10.0, 10.0))));
    runtime.step(frame_input(0.1));
    runtime.step(shell_input(InputEvent::TextInput {
        text: "!".to_string(),
    }));
    let output = runtime.step(frame_input(0.2));
    let upserts = scene_upserts(&output);
    assert!(upserts > 0 && upserts < runtime.scene().len());
}

#[test]
fn input_requests_a_frame_only_for_the_bound_window() {
    let mut runtime = mounted();
    let before = runtime.step(RuntimeInput::Tick(Instant::from_secs_f64(0.0)));
    assert!(requested(&before).is_empty());
    runtime.step(resized_input());
    runtime.step(frame_input(0.0));
    let output = runtime.step(shell_input(press(Point::new(10.0, 10.0))));
    assert!(requested(&output).contains(FrameReason::INPUT));
    assert!(output.shell.iter().any(|command| matches!(
        command,
        ShellCommand::RequestFrame { window, .. } if *window == support::window()
    )));
}

#[test]
fn ime_follows_the_focused_editor() {
    let mut runtime = mounted();
    runtime.step(resized_input());
    runtime.step(frame_input(0.0));
    let focused = runtime.step(shell_input(press(Point::new(190.0, 10.0))));
    let enable = focused.ime.first().expect("focus enables the ime");
    let ImeCommand::Enable(document) = enable else {
        panic!("enable expected, got {enable:?}");
    };
    assert_eq!(document.text, "draft");
    assert_eq!(document.total_len, 5);
    runtime.step(frame_input(0.1));
    let typed = runtime.step(shell_input(InputEvent::TextInput {
        text: "!".to_string(),
    }));
    let update = typed.ime.first().expect("typing updates the ime");
    let ImeCommand::Update(document) = update else {
        panic!("update expected, got {update:?}");
    };
    assert_eq!(document.text, "draft!");
    assert_eq!(document.selection, Utf16Range::new(6, 6));
    let replaced = runtime.step(RuntimeInput::Ime(ImeEvent::Replace {
        range: Utf16Range::new(0, 1),
        text: "D".to_string(),
    }));
    let ImeCommand::Update(document) = replaced.ime.first().unwrap() else {
        panic!("update expected");
    };
    assert_eq!(document.text, "Draft!");
    runtime.tree_mut().set_focused(None);
    let blurred = runtime.step(frame_input(0.2));
    assert_eq!(blurred.ime, vec![ImeCommand::Disable]);
}

#[test]
fn access_tree_is_sent_once_attached_and_only_when_it_changes() {
    let mut runtime = mounted();
    runtime.step(resized_input());
    let detached = runtime.step(frame_input(0.0));
    assert!(detached.access.is_none());
    let attach = runtime.step(RuntimeInput::AccessClient { connected: true });
    assert!(requested(&attach).contains(FrameReason::EXPLICIT));
    let tree = runtime.step(frame_input(0.1)).access.expect("initial tree");
    assert!(!tree.nodes.is_empty());
    assert!(runtime.step(frame_input(0.2)).access.is_none());
    runtime.step(shell_input(press(Point::new(10.0, 10.0))));
    let focused = runtime.step(frame_input(0.3)).access.expect("focus change");
    assert_ne!(focused.focus, tree.focus);
    runtime.step(RuntimeInput::AccessClient { connected: false });
    runtime.step(shell_input(InputEvent::TextInput {
        text: "!".to_string(),
    }));
    assert!(runtime.step(frame_input(0.4)).access.is_none());
}

#[test]
fn clipboard_shortcuts_become_shell_commands() {
    let mut runtime = mounted();
    runtime.step(resized_input());
    runtime.step(frame_input(0.0));
    runtime.step(shell_input(press(Point::new(10.0, 10.0))));
    runtime.step(shell_input(key(Key::Char('a'), Modifiers::CONTROL)));
    let copied = runtime.step(shell_input(key(Key::Char('c'), Modifiers::CONTROL)));
    assert!(copied.shell.contains(&ShellCommand::ClipboardWrite {
        text: "draft".to_string()
    }));
    let pasting = runtime.step(shell_input(key(Key::Char('v'), Modifiers::CONTROL)));
    let request = pasting
        .shell
        .iter()
        .find_map(|command| match command {
            ShellCommand::ClipboardRead { request } => Some(*request),
            _ => None,
        })
        .expect("paste asks the shell for the clipboard");
    let pasted = runtime.step(RuntimeInput::Shell(ShellEvent::Clipboard {
        request,
        text: Some("new".to_string()),
        image: None,
        paths: Vec::new(),
    }));
    assert!(requested(&pasted).contains(FrameReason::INPUT));
    let document = runtime.step(frame_input(0.2));
    let ime_text = document.ime.iter().find_map(|command| match command {
        ImeCommand::Update(document) => Some(document.text.clone()),
        _ => None,
    });
    assert_eq!(ime_text.as_deref(), Some("new"));
}

#[test]
fn frame_wake_follows_motion_in_protocol_time() {
    let mut runtime = mounted();
    support::resize(&mut runtime, 400.0, 300.0, 1.0);
    let motion = runtime.host_mut().motion_mut();
    let handle = motion.spawn(None, MotionValue::Scalar(0.0), tween(0.2), 0.0);
    motion
        .set_target(handle, MotionValue::Scalar(1.0), 0.0)
        .unwrap();

    let now = Instant::from_secs_f64(0.05);
    let wake = support::frame(&mut runtime, 0.05).wake_at;
    assert!(wake.is_some_and(|at| at >= now && at <= Instant::from_secs_f64(0.2)));

    let settled = support::frame(&mut runtime, 1.0);
    assert_eq!(settled.wake_at, None);
}

#[test]
fn a_due_wake_requests_an_animation_frame_on_tick() {
    let mut runtime = mounted();
    support::resize(&mut runtime, 400.0, 300.0, 1.0);
    let motion = runtime.host_mut().motion_mut();
    let handle = motion.spawn(None, MotionValue::Scalar(0.0), tween(0.5), 0.0);
    motion
        .set_target(handle, MotionValue::Scalar(1.0), 0.0)
        .unwrap();
    let wake = support::frame(&mut runtime, 0.0).wake_at.unwrap();
    let due = runtime.step(RuntimeInput::Tick(wake));
    assert!(requested(&due).contains(FrameReason::ANIMATION));
    let again = runtime.step(RuntimeInput::Tick(wake));
    assert!(requested(&again).is_empty());
}

#[test]
fn unconsumed_input_during_motion_requests_only_animation() {
    let mut runtime = mounted();
    support::resize(&mut runtime, 400.0, 300.0, 1.0);
    runtime.step(frame_input(0.0));
    let motion = runtime.host_mut().motion_mut();
    let handle = motion.spawn(None, MotionValue::Scalar(0.0), tween(0.5), 0.0);
    motion
        .set_target(handle, MotionValue::Scalar(1.0), 0.0)
        .unwrap();

    let output = runtime.step(shell_input(moved(Point::new(390.0, 290.0))));
    let reason = requested(&output);
    assert!(reason.contains(FrameReason::ANIMATION));
    assert!(!reason.contains(FrameReason::INPUT));

    runtime.step(frame_input(2.0));
    let idle = runtime.step(shell_input(moved(Point::new(390.0, 290.0))));
    assert!(requested(&idle).is_empty());
}

#[test]
fn time_never_runs_backwards() {
    let mut runtime = mounted();
    support::tick(&mut runtime, 2.0);
    support::tick(&mut runtime, 1.0);
    assert_eq!(runtime.now_secs(), 2.0);
}

#[path = "runtime/selection-sources.rs"]
mod selection_sources;

#[path = "runtime/controls.rs"]
mod controls;

#[path = "runtime/incremental.rs"]
mod incremental;
#[path = "runtime/paint.rs"]
mod paint;
