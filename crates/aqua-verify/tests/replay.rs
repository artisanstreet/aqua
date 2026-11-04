use aqua_core::{
    DisplayItem, InputEvent, Key, KeyEvent, Modifiers, Oklch, Point, PointerButton, PointerEvent,
    PointerKind, ScrollDelta,
};
use aqua_protocol::{
    first_divergence, replay, Instant, Machine, PixelSize, Recorded, Recorder, RequestId,
    RuntimeInput, RuntimeOutput, ShellEvent, Utf16Range, WindowId,
};
use aqua_protocol::{ImeEvent, RenderCommand, RenderInput};
use aqua_runtime::{RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_transport::lower::lower_items;
use aqua_ui::element::ui::{Box as UiBox, Scroll, Text as UiText};
use aqua_ui::style::{px, Axis, Style};
use aqua_verify::fingerprint;

const SCRIPT_LENGTH: usize = 400;

fn window() -> WindowId {
    WindowId::new(1).unwrap()
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn unit(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
}

fn mounted() -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Scroll::new()
                .style(Style::new().width(px(360.0)).height(px(280.0)))
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .axis(Axis::Column)
                                .width(px(340.0))
                                .flex_shrink(0.0),
                        )
                        .children((0..40).map(|row| {
                            match row % 5 {
                                0 => UiText::builder()
                                    .arg(format!("field {row}"))
                                    .editable(true)
                                    .style(Style::new().width(px(320.0)).height(px(32.0)))
                                    .build(),
                                _ => UiBox::new()
                                    .fill(Oklch::from_hex("#223344").unwrap())
                                    .radius(4.0)
                                    .style(Style::new().width(px(320.0)).height(px(30.0)))
                                    .child(
                                        UiText::builder()
                                            .arg(format!("row {row} with some words to wrap"))
                                            .wrap(true)
                                            .style(Style::new().width(px(300.0)))
                                            .build(),
                                    )
                                    .build(),
                            }
                        }))
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

fn shell(event: InputEvent) -> RuntimeInput {
    RuntimeInput::Shell(ShellEvent::Input {
        window: window(),
        event,
    })
}

fn script(seed: u64) -> Vec<RuntimeInput> {
    let mut rng = Rng(seed);
    let mut now = 0.0f64;
    let mut inputs = vec![RuntimeInput::Shell(ShellEvent::Resized {
        window: window(),
        size: PixelSize {
            width: 540,
            height: 420,
        },
        scale: 1.5,
    })];
    let keys = [
        Key::Char('a'),
        Key::Char('c'),
        Key::Char('v'),
        Key::Char('z'),
    ];
    (0..SCRIPT_LENGTH).for_each(|_| {
        now += 0.004 + f64::from(rng.unit()) * 0.03;
        let at = Point::new(rng.unit() * 360.0, rng.unit() * 280.0);
        let input = match rng.below(14) {
            0..=2 => shell(InputEvent::PointerMove(pointer(at, 0))),
            3 => shell(InputEvent::PointerDown(
                pointer(at, 1),
                PointerButton::Primary,
            )),
            4 => shell(InputEvent::PointerUp(
                pointer(at, 0),
                PointerButton::Primary,
            )),
            5 | 6 => shell(InputEvent::Scroll {
                at,
                delta: ScrollDelta {
                    x: 0.0,
                    y: (rng.unit() - 0.5) * 160.0,
                },
            }),
            7 => shell(InputEvent::TextInput {
                text: ["x", "é", "wor", "日本"][rng.below(4) as usize].to_string(),
            }),
            8 => shell(InputEvent::KeyDown(KeyEvent {
                key: keys[rng.below(4) as usize],
                modifiers: Modifiers::CONTROL,
                repeat: false,
            })),
            9 => RuntimeInput::Ime(ImeEvent::Replace {
                range: Utf16Range::new(0, rng.below(3) as u32),
                text: "ab".to_string(),
            }),
            10 => RuntimeInput::AccessClient {
                connected: rng.below(2) == 0,
            },
            11 => RuntimeInput::Shell(ShellEvent::Clipboard {
                request: RequestId(rng.below(4)),
                text: Some("clip".to_string()),
                image: None,
                paths: Vec::new(),
            }),
            12 => RuntimeInput::Tick(Instant::from_secs_f64(now)),
            _ => RuntimeInput::Shell(ShellEvent::Frame {
                window: window(),
                presentation: Instant::from_secs_f64(now),
            }),
        };
        inputs.push(input);
        inputs.push(RuntimeInput::Shell(ShellEvent::Frame {
            window: window(),
            presentation: Instant::from_secs_f64(now + 0.001),
        }));
    });
    inputs
}

fn scene_items(runtime: &Runtime) -> Vec<DisplayItem> {
    runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect()
}

fn presented_plan_hash(log: &[Recorded<RuntimeInput, RuntimeOutput>]) -> (u64, usize) {
    let mut side = RenderSide::new();
    let mut hashes = Vec::new();
    log.iter().enumerate().for_each(|(index, entry)| {
        let commands: Vec<RenderCommand> = entry.output.render.clone();
        if commands.is_empty() {
            return;
        }
        side.step(RenderInput {
            commands,
            feedback: Vec::new(),
            present_at: Instant::from_secs_f64(index as f64 * 0.002),
        });
        let (_, scale) = side
            .viewport()
            .unwrap_or((aqua_core::Size { w: 0.0, h: 0.0 }, 1.0));
        let lowered = lower_items(side.scene().ordered().map(|(_, item)| item), scale);
        hashes.push(fingerprint::of(&lowered.commands));
    });
    (fingerprint::of(&hashes), hashes.len())
}

fn run_recorded(seed: u64) -> (Runtime, Vec<Recorded<RuntimeInput, RuntimeOutput>>) {
    let mut recorder = Recorder::new(mounted());
    script(seed).into_iter().for_each(|input| {
        recorder.step(input);
    });
    let log = recorder.log().to_vec();
    let mut runtime = mounted();
    let replayed = replay(&mut runtime, log.iter().map(|entry| entry.input.clone()));
    assert_eq!(first_divergence(&log, &replayed), None, "seed {seed}");
    (runtime, log)
}

#[test]
fn replaying_randomised_sessions_reproduces_outputs_scene_and_plan_hashes() {
    for seed in [1u64, 7, 0xA9_0A, 0xD3D1_2D1F, 99_991] {
        let (replayed, log) = run_recorded(seed);
        let mut recorder = Recorder::new(mounted());
        script(seed).into_iter().for_each(|input| {
            recorder.step(input);
        });
        assert_eq!(
            fingerprint::of(&scene_items(recorder.machine())),
            fingerprint::of(&scene_items(&replayed)),
            "seed {seed}"
        );
        let second_log = recorder.into_log();
        assert_eq!(
            presented_plan_hash(&log),
            presented_plan_hash(&second_log),
            "seed {seed}"
        );
        assert!(
            log.iter().any(|entry| !entry.output.render.is_empty()),
            "seed {seed}: the session must produce scene output"
        );
        assert!(
            log.iter().any(|entry| !entry.output.shell.is_empty()),
            "seed {seed}"
        );
    }
}

#[test]
fn different_sessions_do_not_collide_on_scene_or_plan_hashes() {
    let (first, first_log) = run_recorded(3);
    let (second, second_log) = run_recorded(4);
    assert_ne!(
        fingerprint::of(&scene_items(&first)),
        fingerprint::of(&scene_items(&second))
    );
    assert_ne!(
        presented_plan_hash(&first_log).0,
        presented_plan_hash(&second_log).0
    );
}

#[test]
fn an_altered_input_is_located_at_the_step_that_changed() {
    let (_, log) = run_recorded(11);
    let mut inputs: Vec<_> = log.iter().map(|entry| entry.input.clone()).collect();
    let target = 100;
    inputs[target] = RuntimeInput::Shell(ShellEvent::Resized {
        window: window(),
        size: PixelSize {
            width: 900,
            height: 700,
        },
        scale: 2.0,
    });
    let mut fresh = mounted();
    let replayed = replay(&mut fresh, inputs);
    let divergence = first_divergence(&log, &replayed).expect("diverges");
    assert!((target..target + 3).contains(&divergence), "{divergence}");
}
