use std::time::Duration;

use aqua_protocol::{
    ImeEvent, Instant, Machine, Pace, RuntimeInput, RuntimeOutput, ShellEvent, Timeline, Utf16Range,
    WindowId,
};

#[derive(Default)]
struct Probe {
    ticks: Vec<u64>,
    frames: Vec<u64>,
    ime: u32,
}

impl Machine for Probe {
    type Input = RuntimeInput;
    type Output = RuntimeOutput;

    fn step(&mut self, input: RuntimeInput) -> RuntimeOutput {
        match input {
            RuntimeInput::Tick(at) => self.ticks.push(at.as_nanos()),
            RuntimeInput::Shell(ShellEvent::Frame { presentation, .. }) => {
                self.frames.push(presentation.as_nanos())
            }
            RuntimeInput::Ime(_) => self.ime += 1,
            _ => {}
        }
        RuntimeOutput {
            wake_at: Some(Instant::from_nanos(self.ticks.len() as u64)),
            ..RuntimeOutput::default()
        }
    }
}

fn ms(value: u64) -> Instant {
    Instant::from_nanos(value * 1_000_000)
}

fn window() -> WindowId {
    WindowId::new(1).unwrap()
}

fn ime() -> RuntimeInput {
    RuntimeInput::Ime(ImeEvent::Select {
        range: Utf16Range::new(0, 0),
        reversed: false,
    })
}

fn timeline() -> Timeline<Probe> {
    Timeline::new(Probe::default)
}

#[test]
fn real_time_passes_through_at_unit_rate() {
    let mut line = timeline();
    line.step(RuntimeInput::Tick(ms(1000)));
    line.step(RuntimeInput::Tick(ms(1016)));
    line.step(RuntimeInput::Tick(ms(1032)));
    assert_eq!(line.machine().ticks, vec![0, 16_000_000, 32_000_000]);
}

#[test]
fn a_paused_timeline_freezes_machine_time_and_still_delivers_other_inputs() {
    let mut line = timeline();
    line.step(RuntimeInput::Tick(ms(1000)));
    line.step(RuntimeInput::Tick(ms(1100)));
    line.pause();
    line.step(RuntimeInput::Tick(ms(1500)));
    line.step(ime());
    line.step(RuntimeInput::Shell(ShellEvent::Frame {
        window: window(),
        presentation: ms(1600),
    }));
    assert_eq!(line.machine().ticks, vec![0, 100_000_000, 100_000_000]);
    assert_eq!(line.machine().frames, vec![100_000_000]);
    assert_eq!(line.machine().ime, 1);
    line.resume();
    line.step(RuntimeInput::Tick(ms(1616)));
    assert_eq!(line.machine().ticks.last(), Some(&116_000_000));
}

#[test]
fn slow_motion_scales_elapsed_time() {
    let mut line = timeline();
    line.slow(0.25);
    line.step(RuntimeInput::Tick(ms(0)));
    line.step(RuntimeInput::Tick(ms(400)));
    assert_eq!(line.machine().ticks, vec![0, 100_000_000]);
    assert_eq!(line.pace(), Pace::Running { rate: 0.25 });
}

#[test]
fn stepping_a_paused_timeline_advances_exactly_the_requested_duration() {
    let mut line = timeline();
    line.pause();
    line.advance(Duration::from_millis(16), &[window()]);
    line.advance(Duration::from_millis(16), &[window()]);
    assert_eq!(line.now(), ms(32));
    assert_eq!(line.machine().ticks, vec![16_000_000, 32_000_000]);
    assert_eq!(line.machine().frames, vec![16_000_000, 32_000_000]);
}

#[test]
fn seeking_back_rebuilds_the_machine_and_seeking_forward_replays_the_recording() {
    let mut line = timeline();
    for step in 0..10 {
        line.step(RuntimeInput::Tick(ms(1000 + step * 10)));
        line.step(ime());
    }
    assert_eq!(line.machine().ticks.len(), 10);
    line.seek(ms(40));
    assert_eq!(line.machine().ticks, vec![0, 10_000_000, 20_000_000, 30_000_000, 40_000_000]);
    assert_eq!(line.machine().ime, 5);
    assert_eq!(line.now(), ms(40));
    line.seek(ms(90));
    assert_eq!(line.machine().ticks.len(), 10);
    assert_eq!(line.machine().ime, 10);
    assert_eq!(line.cursor(), 20);
    assert_eq!(line.recorded(), 20);
}

#[test]
fn live_input_after_a_seek_discards_the_rewound_future() {
    let mut line = timeline();
    for step in 0..6 {
        line.step(RuntimeInput::Tick(ms(step * 10)));
    }
    line.seek(ms(20));
    line.pause();
    line.step(ime());
    assert_eq!(line.recorded(), 4);
    assert_eq!(line.machine().ticks, vec![0, 10_000_000, 20_000_000]);
    assert_eq!(line.machine().ime, 1);
}

#[test]
fn seeking_before_the_first_entry_returns_to_the_initial_machine() {
    let mut line = timeline();
    line.step(RuntimeInput::Tick(ms(5)));
    line.step(RuntimeInput::Tick(ms(50)));
    line.seek(Instant::ZERO);
    assert_eq!(line.machine().ticks, vec![0]);
    let output = line.seek(Instant::ZERO);
    assert!(output.is_idle());
}
