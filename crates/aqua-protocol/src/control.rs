use std::time::Duration;

use crate::machine::{Input, Machine, Output};
use crate::shell::ShellEvent;
use crate::time::Instant;
use crate::window::WindowId;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pace {
    Running { rate: f64 },
    Paused,
}

impl Pace {
    pub const REAL_TIME: Pace = Pace::Running { rate: 1.0 };

    fn rate(self) -> f64 {
        match self {
            Pace::Running { rate } => rate.max(0.0),
            Pace::Paused => 0.0,
        }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    input: Input,
    at: Instant,
}

fn stamp(input: &Input) -> Option<Instant> {
    match input {
        Input::Tick(at) => Some(*at),
        Input::Shell(ShellEvent::Frame { presentation, .. }) => Some(*presentation),
        _ => None,
    }
}

fn retime(input: Input, to: Instant) -> Input {
    match input {
        Input::Tick(_) => Input::Tick(to),
        Input::Shell(ShellEvent::Frame { window, .. }) => Input::Shell(ShellEvent::Frame {
            window,
            presentation: to,
        }),
        other => other,
    }
}

pub struct Timeline<M> {
    factory: Box<dyn FnMut() -> M>,
    machine: M,
    entries: Vec<Entry>,
    cursor: usize,
    pace: Pace,
    anchor: Option<Instant>,
    now: Instant,
}

impl<M> Timeline<M>
where
    M: Machine<Input = Input, Output = Output>,
{
    pub fn new(mut factory: impl FnMut() -> M + 'static) -> Self {
        let machine = factory();
        Self {
            factory: Box::new(factory),
            machine,
            entries: Vec::new(),
            cursor: 0,
            pace: Pace::REAL_TIME,
            anchor: None,
            now: Instant::ZERO,
        }
    }

    pub fn machine(&self) -> &M {
        &self.machine
    }

    pub fn now(&self) -> Instant {
        self.now
    }

    pub fn pace(&self) -> Pace {
        self.pace
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn recorded(&self) -> usize {
        self.entries.len()
    }

    pub fn inputs(&self) -> impl Iterator<Item = &Input> {
        self.entries.iter().map(|entry| &entry.input)
    }

    pub fn set_pace(&mut self, pace: Pace) {
        self.pace = pace;
    }

    pub fn pause(&mut self) {
        self.set_pace(Pace::Paused);
    }

    pub fn resume(&mut self) {
        self.set_pace(Pace::REAL_TIME);
    }

    pub fn slow(&mut self, rate: f64) {
        self.set_pace(Pace::Running { rate });
    }

    pub fn advance(&mut self, by: Duration, frames: &[WindowId]) -> Output {
        self.now += by;
        let mut output = self.apply(Input::Tick(self.now));
        for window in frames {
            output.merge(self.apply(Input::Shell(ShellEvent::Frame {
                window: *window,
                presentation: self.now,
            })));
        }
        output
    }

    pub fn seek(&mut self, target: Instant) -> Output {
        let landing = self.entries.partition_point(|entry| entry.at <= target);
        let mut output = Output::default();
        let from = match landing >= self.cursor {
            true => self.cursor,
            false => {
                self.machine = (self.factory)();
                0
            }
        };
        for index in from..landing {
            let input = self.entries[index].input.clone();
            output.merge(self.machine.step(input));
        }
        self.cursor = landing;
        self.now = match landing {
            0 => Instant::ZERO,
            n => self.entries[n - 1].at,
        };
        self.anchor = None;
        output
    }

    fn map(&mut self, real: Instant) -> Instant {
        match self.anchor {
            None => self.anchor = Some(real),
            Some(anchor) if real > anchor => {
                let elapsed = (real - anchor).as_secs_f64() * self.pace.rate();
                self.now += Duration::from_secs_f64(elapsed);
                self.anchor = Some(real);
            }
            Some(_) => {}
        }
        self.now
    }

    fn apply(&mut self, input: Input) -> Output {
        self.entries.truncate(self.cursor);
        let output = self.machine.step(input.clone());
        self.entries.push(Entry {
            input,
            at: self.now,
        });
        self.cursor += 1;
        output
    }
}

impl<M> Machine for Timeline<M>
where
    M: Machine<Input = Input, Output = Output>,
{
    type Input = Input;
    type Output = Output;

    fn step(&mut self, input: Input) -> Output {
        let input = match stamp(&input) {
            Some(real) => {
                let virtual_time = self.map(real);
                retime(input, virtual_time)
            }
            None => input,
        };
        self.apply(input)
    }
}
