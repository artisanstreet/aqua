use aqua_core::geometry::Rect;

use crate::access::{AccessAction, AccessTreeUpdate};
use crate::ime::{ImeCommand, ImeEvent};
use crate::render::{GpuEvent, RenderCommand, RenderEvent};
use crate::shell::{ShellCommand, ShellEvent};
use crate::time::Instant;

pub trait Machine {
    type Input;
    type Output;

    fn step(&mut self, input: Self::Input) -> Self::Output;
}

#[derive(Clone, Debug)]
pub enum Input {
    Shell(ShellEvent),
    Ime(ImeEvent),
    Access(AccessAction),
    AccessClient { connected: bool },
    Gpu(GpuEvent),
    Render(RenderEvent),
    Tick(Instant),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub shell: Vec<ShellCommand>,
    pub ime: Vec<ImeCommand>,
    pub access: Option<AccessTreeUpdate>,
    pub render: Vec<RenderCommand>,
    pub wake_at: Option<Instant>,
    pub ime_area: Option<Rect>,
}

impl Output {
    pub fn is_idle(&self) -> bool {
        self.shell.is_empty()
            && self.ime.is_empty()
            && self.access.is_none()
            && self.render.is_empty()
            && self.ime_area.is_none()
    }

    pub fn merge(&mut self, other: Output) {
        self.shell.extend(other.shell);
        self.ime.extend(other.ime);
        self.access = other.access.or(self.access.take());
        self.render.extend(other.render);
        self.wake_at = match (self.wake_at, other.wake_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.ime_area = other.ime_area.or(self.ime_area);
    }
}

#[derive(Clone, Debug)]
pub struct Recorded<I, O> {
    pub input: I,
    pub output: O,
}

#[derive(Debug)]
pub struct Recorder<M: Machine> {
    machine: M,
    log: Vec<Recorded<M::Input, M::Output>>,
}

impl<M> Recorder<M>
where
    M: Machine,
    M::Input: Clone,
    M::Output: Clone,
{
    pub fn new(machine: M) -> Self {
        Self {
            machine,
            log: Vec::new(),
        }
    }

    pub fn machine(&self) -> &M {
        &self.machine
    }

    pub fn log(&self) -> &[Recorded<M::Input, M::Output>] {
        &self.log
    }

    pub fn into_log(self) -> Vec<Recorded<M::Input, M::Output>> {
        self.log
    }
}

impl<M> Machine for Recorder<M>
where
    M: Machine,
    M::Input: Clone,
    M::Output: Clone,
{
    type Input = M::Input;
    type Output = M::Output;

    fn step(&mut self, input: M::Input) -> M::Output {
        let output = self.machine.step(input.clone());
        self.log.push(Recorded {
            input,
            output: output.clone(),
        });
        output
    }
}

pub fn replay<M, I>(machine: &mut M, inputs: I) -> Vec<M::Output>
where
    M: Machine,
    I: IntoIterator<Item = M::Input>,
{
    inputs
        .into_iter()
        .map(|input| machine.step(input))
        .collect()
}

pub fn first_divergence<I, O: PartialEq>(log: &[Recorded<I, O>], replayed: &[O]) -> Option<usize> {
    log.iter()
        .zip(replayed)
        .position(|(recorded, output)| recorded.output != *output)
        .or_else(|| (log.len() != replayed.len()).then_some(log.len().min(replayed.len())))
}
