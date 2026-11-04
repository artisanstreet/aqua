use aqua_core::{DisplayItem, InputEvent, Point, ScrollDelta};
use aqua_motion::{Easing, MotionKind, MotionValue, Property};
use aqua_protocol::{
    Instant, Machine, RenderCommand, RenderInput, RuntimeInput, RuntimeOutput, ShellEvent, WindowId,
};
use aqua_runtime::{PopupOptions, RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Scroll};
use aqua_ui::overlay_host::PopupMotion;
use aqua_ui::style::{px, Axis, Style};

const RATES: [f64; 3] = [60.0, 144.0, 240.0];
const REFERENCE_RATE: f64 = 1440.0;
const CURVATURE_BOUND: f32 = 0.25;

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

    fn unit(&mut self) -> f64 {
        (self.next() % 100_000) as f64 / 100_000.0
    }
}

struct Rig {
    runtime: Runtime,
    render: RenderSide,
}

impl Rig {
    fn new(runtime: Runtime) -> Self {
        Self {
            runtime,
            render: RenderSide::new(),
        }
    }

    fn deliver(&mut self, commands: Vec<RenderCommand>, secs: f64) {
        let output = self.render.step(RenderInput {
            commands,
            feedback: Vec::new(),
            present_at: Instant::from_secs_f64(secs),
        });
        output.events.into_iter().for_each(|event| {
            self.runtime.step(RuntimeInput::Render(event));
        });
    }

    fn app_frame(&mut self, secs: f64) -> RuntimeOutput {
        let out = self.runtime.step(RuntimeInput::Shell(ShellEvent::Frame {
            window: window(),
            presentation: Instant::from_secs_f64(secs),
        }));
        self.deliver(out.render.clone(), secs);
        out
    }

    fn render_only(&mut self, secs: f64) {
        self.deliver(Vec::new(), secs);
    }

    fn wheel(&mut self, secs: f64, delta: f32) {
        self.runtime.step(RuntimeInput::Shell(ShellEvent::Input {
            window: window(),
            event: InputEvent::Scroll {
                at: Point::new(20.0, 20.0),
                delta: ScrollDelta { x: 0.0, y: delta },
            },
        }));
        self.app_frame(secs);
    }

    fn translation(&self) -> f32 {
        self.render
            .scene()
            .ordered()
            .find_map(|(_, item)| match item {
                DisplayItem::PushTransform(t) => Some(t.ty),
                _ => None,
            })
            .unwrap_or(0.0)
    }

    fn offset(&self) -> f32 {
        let root = self.runtime.tree().root().unwrap();
        self.runtime.tree().scroll_offset(root).unwrap().y
    }

    fn visual(&self) -> f32 {
        self.translation() - self.offset()
    }

    fn opacities(&self) -> Vec<f32> {
        self.render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::PushOpacity(o) => Some(*o),
                _ => None,
            })
            .collect()
    }
}

fn scroller() -> Rig {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Scroll::new()
                .style(Style::new().width(px(200.0)).height(px(100.0)))
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .axis(Axis::Column)
                                .width(px(200.0))
                                .height(px(20000.0))
                                .flex_shrink(0.0),
                        )
                        .build(),
                )
                .build()
        })
        .unwrap();
    runtime.step(RuntimeInput::Shell(ShellEvent::Resized {
        window: window(),
        size: aqua_protocol::PixelSize {
            width: 300,
            height: 200,
        },
        scale: 1.0,
    }));
    Rig::new(runtime)
}

#[derive(Clone, Copy, Debug)]
struct Notch {
    at: f64,
    delta: f32,
}

fn notches(seed: u64) -> Vec<Notch> {
    let mut rng = Rng(seed);
    let mut at = 0.5;
    (0..10)
        .map(|_| {
            at += 0.01 + rng.unit() * 0.3;
            Notch {
                at,
                delta: 16.0 * (((rng.unit() - 0.3) * 6.0).round() as f32),
            }
        })
        .collect()
}

struct Sample {
    time: f64,
    visual: f32,
    translation: f32,
    wheel: bool,
}

fn run_at(notches: &[Notch], rate: f64, horizon: f64) -> Vec<Sample> {
    let mut rig = scroller();
    rig.app_frame(0.0);
    let steps = (horizon * rate).round() as usize;
    let mut pending = notches.iter().peekable();
    let mut samples = Vec::with_capacity(steps);
    (1..=steps).for_each(|step| {
        let time = step as f64 / rate;
        let mut wheel = false;
        while let Some(notch) = pending.next_if(|notch| notch.at <= time) {
            rig.wheel(notch.at, notch.delta);
            wheel = true;
        }
        rig.render_only(time);
        samples.push(Sample {
            time,
            visual: rig.visual(),
            translation: rig.translation(),
            wheel,
        });
    });
    samples
}

#[test]
fn scroll_glide_has_no_visual_jump_at_wheel_events_or_between_frames() {
    for seed in 1..=12u64 {
        let schedule = notches(seed);
        let horizon = (schedule.last().unwrap().at + 3.0).ceil();
        let reference = run_at(&schedule, REFERENCE_RATE, horizon);
        let reference_step = reference
            .windows(2)
            .filter(|pair| !pair[1].wheel)
            .map(|pair| (pair[1].visual - pair[0].visual).abs())
            .fold(0.0f32, f32::max);
        let wheel_jump = reference
            .windows(2)
            .filter(|pair| pair[1].wheel)
            .map(|pair| (pair[1].visual - pair[0].visual).abs())
            .fold(0.0f32, f32::max);
        assert!(
            wheel_jump <= reference_step.max(0.5) * 2.5 + 0.5,
            "seed {seed}: a wheel event jumped the content by {wheel_jump} (glide step {reference_step})"
        );
        let worst_curvature = reference
            .windows(3)
            .filter(|triple| !triple[1].wheel && !triple[2].wheel)
            .map(|triple| (triple[2].visual - 2.0 * triple[1].visual + triple[0].visual).abs())
            .fold(0.0f32, f32::max);
        eprintln!("seed {seed} worst curvature {worst_curvature}");
        assert!(worst_curvature <= CURVATURE_BOUND, "seed {seed}");
        let last = reference.last().unwrap();
        assert_eq!(
            last.translation, 0.0,
            "seed {seed}: the glide settles exactly"
        );
    }
}

#[test]
fn scroll_glide_is_independent_of_the_presentation_rate() {
    for seed in 1..=8u64 {
        let schedule = notches(seed);
        let horizon = (schedule.last().unwrap().at + 3.0).ceil();
        let reference = run_at(&schedule, REFERENCE_RATE, horizon);
        for rate in RATES {
            let coarse = run_at(&schedule, rate, horizon);
            let stride = (REFERENCE_RATE / rate).round() as usize;
            coarse.iter().enumerate().for_each(|(index, sample)| {
                let matching = &reference[(index + 1) * stride - 1];
                assert!(
                    (sample.time - matching.time).abs() < 1e-6,
                    "{rate} Hz sample {index}"
                );
                assert!(
                    (sample.visual - matching.visual).abs() < 0.05,
                    "seed {seed} {rate} Hz t={}: {} vs {}",
                    sample.time,
                    sample.visual,
                    matching.visual
                );
            });
        }
    }
}

fn popup_rig() -> (Rig, aqua_ui::overlay::OverlayId) {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime
        .mount(|_| {
            UiBox::new()
                .child(
                    UiBox::new()
                        .style(Style::new().width(px(10.0)).height(px(10.0)))
                        .build(),
                )
                .build()
        })
        .unwrap();
    runtime.step(RuntimeInput::Shell(ShellEvent::Resized {
        window: window(),
        size: aqua_protocol::PixelSize {
            width: 300,
            height: 200,
        },
        scale: 1.0,
    }));
    runtime.build_frame().unwrap();
    let owner = runtime.tree().root().unwrap();
    let popup = runtime
        .open_popup(
            owner,
            UiBox::new()
                .style(
                    Style::new()
                        .width(px(80.0))
                        .height(px(40.0))
                        .background(aqua_core::Oklch::from_hex("#334455").unwrap()),
                )
                .build(),
            PopupOptions::menu().at(Point::new(10.0, 20.0)),
        )
        .unwrap();
    (Rig::new(runtime), popup)
}

fn fade(duration_secs: f64, easing: Easing) -> PopupMotion {
    PopupMotion {
        property: Property::Opacity,
        from: MotionValue::Scalar(0.0),
        to: MotionValue::Scalar(1.0),
        kind: MotionKind::Tween {
            duration_secs,
            easing,
        },
    }
}

#[test]
fn popup_first_frame_never_flashes_the_target_and_fades_monotonically_to_exactly_one() {
    let easings = [Easing::Linear, Easing::EaseOut, Easing::EaseInOut];
    for (index, easing) in easings.into_iter().enumerate() {
        for rate in RATES {
            let duration = 0.12 + 0.05 * index as f64;
            let (mut rig, popup) = popup_rig();
            rig.app_frame(0.0);
            assert!(rig.runtime.animate_popup(popup, fade(duration, easing)));
            rig.app_frame(1.0);
            assert_eq!(
                rig.opacities(),
                vec![0.0],
                "{easing:?} {rate} Hz first frame"
            );
            let mut last = 0.0;
            let steps = ((duration + 0.3) * rate) as usize;
            (1..=steps).for_each(|step| {
                rig.render_only(1.0 + step as f64 / rate);
                let opacity = rig.opacities()[0];
                assert!(
                    opacity >= last - 1e-6 && opacity <= 1.0,
                    "{easing:?} {rate} Hz step {step}: {opacity} after {last}"
                );
                assert!(
                    opacity - last <= (1.5 / (duration * rate)) as f32 * 3.0 + 1e-3,
                    "{easing:?} {rate} Hz step {step}: jumped from {last} to {opacity}"
                );
                last = opacity;
            });
            assert_eq!(last, 1.0, "{easing:?} {rate} Hz settles exactly");
            assert!(rig.render.tracks().is_idle());
        }
    }
}

#[test]
fn closing_a_popup_mid_fade_leaves_no_track_group_or_residual_opacity() {
    let mut rng = Rng(5);
    for _ in 0..10 {
        let (mut rig, popup) = popup_rig();
        rig.app_frame(0.0);
        rig.runtime.animate_popup(popup, fade(0.2, Easing::Linear));
        rig.app_frame(1.0);
        let cut = 1 + (rng.unit() * 20.0) as usize;
        (1..=cut).for_each(|step| rig.render_only(1.0 + step as f64 / 120.0));
        assert!(rig.runtime.close_popup(popup));
        rig.app_frame(1.5);
        rig.render_only(1.6);
        assert_eq!(rig.render.tracks().len(), 0);
        assert!(rig.opacities().is_empty());
        assert!(rig.render.tracks().is_idle());
    }
}

#[test]
fn first_mount_and_idle_frames_start_no_motion() {
    let mut rig = scroller();
    (0..30).for_each(|frame| {
        rig.app_frame(frame as f64 / 60.0);
        rig.render_only(frame as f64 / 60.0 + 0.001);
    });
    assert_eq!(rig.render.tracks().len(), 0);
    assert!(rig.render.tracks().is_idle());
    assert_eq!(rig.translation(), 0.0);
}
