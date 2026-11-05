use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant as WallInstant};

use aqua_protocol::{
    Feedback, FrameId, Instant, RenderCommand, RenderEvent, RenderInput, RenderWake,
};
use aqua_runtime::RenderSide;
use aqua_transport::{Notify, Receiver, RecvError, Sender};

use super::host::{Clock, FrameTarget, RenderStats, Repaint, TargetInit, epoch_ns};

const DRAIN_LIMIT: usize = 4096;
const MAX_PENDING_POLLS: u32 = 60;
const REMOVE_TIMEOUT: Duration = Duration::from_secs(3);
const ANIMATION_PERIOD: Duration = Duration::from_millis(15);
const HEAVY_ANIMATION_PERIOD: Duration = Duration::from_millis(33);

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct Signal {
    raised: Mutex<bool>,
    condvar: Condvar,
}

impl Signal {
    fn notify(&self) {
        *locked(&self.raised) = true;
        self.condvar.notify_one();
    }

    fn wait(&self, timeout: Option<Duration>) {
        let mut raised = locked(&self.raised);
        if !*raised {
            raised = match timeout {
                Some(timeout) => {
                    self.condvar
                        .wait_timeout(raised, timeout)
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .0
                }
                None => self
                    .condvar
                    .wait(raised)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            };
        }
        *raised = false;
    }
}

pub struct NewSlot {
    pub commands: Receiver<RenderCommand>,
    pub events: Sender<RenderEvent>,
    pub init: TargetInit,
    pub clock: Clock,
    pub interval: Duration,
    pub stats: Arc<RenderStats>,
}

enum Control {
    Add(u64, Box<NewSlot>),
    Remove(u64, SyncSender<Option<RenderSide>>),
    Shutdown,
}

#[derive(Default)]
struct Shared {
    signal: Signal,
    control: Mutex<VecDeque<Control>>,
    next: AtomicU64,
}

impl Shared {
    fn push(&self, control: Control) {
        locked(&self.control).push_back(control);
        self.signal.notify();
    }
}

#[derive(Clone)]
pub struct ServiceHandle {
    shared: Arc<Shared>,
}

impl ServiceHandle {
    pub fn add(&self, slot: NewSlot) -> u64 {
        let id = self.shared.next.fetch_add(1, Ordering::Relaxed);
        self.shared.push(Control::Add(id, Box::new(slot)));
        id
    }

    pub fn remove(&self, slot: u64) -> Option<RenderSide> {
        let (reply, receiver) = sync_channel(1);
        self.shared.push(Control::Remove(slot, reply));
        receiver.recv_timeout(REMOVE_TIMEOUT).ok().flatten()
    }

    pub fn wake(&self) {
        self.shared.signal.notify();
    }

    pub fn notifier(&self) -> Notify {
        let shared = self.shared.clone();
        Arc::new(move || shared.signal.notify())
    }
}

pub struct RenderService {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl RenderService {
    pub fn new() -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let serving = shared.clone();
        let thread = std::thread::Builder::new()
            .name("aqua-render".into())
            .spawn(move || serve(&serving))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub fn handle(&self) -> ServiceHandle {
        ServiceHandle {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for RenderService {
    fn drop(&mut self) {
        self.shared.push(Control::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

enum Stage {
    Pending(TargetInit),
    Running(Box<dyn FrameTarget>),
    Dead { reported: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Settled,
    Continuous,
    At(WallInstant),
}

enum Status {
    Closed,
    Ran,
    Waiting(Option<WallInstant>),
}

struct Slot {
    id: u64,
    commands: Receiver<RenderCommand>,
    events: Sender<RenderEvent>,
    side: RenderSide,
    stage: Stage,
    clock: Clock,
    interval: Duration,
    stats: Arc<RenderStats>,
    inbox: Vec<RenderCommand>,
    feedback: Vec<Feedback>,
    wake: RenderWake,
    pending: Pending,
    polls: u32,
    quiet: bool,
    not_before: Option<WallInstant>,
    presented_last: bool,
    paced: bool,
    seen_arrivals: u64,
    last_frame: FrameId,
    hidden_seen: bool,
    cause: super::runs::RunCause,
}

fn thread_token() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    hasher.finish() | 1
}

fn drain(receiver: &mut Receiver<RenderCommand>, into: &mut Vec<RenderCommand>) -> bool {
    loop {
        if into.len() >= DRAIN_LIMIT {
            return false;
        }
        match receiver.try_recv() {
            Ok(command) => into.push(command),
            Err(RecvError::Disconnected) => return true,
            Err(_) => return false,
        }
    }
}

impl Slot {
    fn new(id: u64, new: NewSlot) -> Self {
        Self {
            id,
            commands: new.commands,
            events: new.events,
            side: RenderSide::new(),
            stage: Stage::Pending(new.init),
            clock: new.clock,
            interval: new.interval,
            stats: new.stats,
            inbox: Vec::new(),
            feedback: Vec::new(),
            wake: RenderWake::NextFrame,
            pending: Pending::Settled,
            polls: 0,
            quiet: false,
            not_before: None,
            presented_last: false,
            paced: false,
            seen_arrivals: 0,
            last_frame: 0,
            hidden_seen: false,
            cause: super::runs::RunCause::default(),
        }
    }

    fn present_time(&self) -> Instant {
        (self.clock)() + self.interval
    }

    fn start(&mut self) -> bool {
        match std::mem::replace(&mut self.stage, Stage::Dead { reported: false }) {
            Stage::Pending(init) => match init() {
                Ok(target) => {
                    self.paced = target.paced();
                    self.stage = Stage::Running(target);
                    true
                }
                Err(message) => {
                    eprintln!("aqua: render target failed: {message}");
                    false
                }
            },
            Stage::Running(target) => {
                self.stage = Stage::Running(target);
                true
            }
            dead @ Stage::Dead { .. } => {
                self.stage = dead;
                false
            }
        }
    }

    fn wake_deadline(&self, now: WallInstant) -> Option<WallInstant> {
        match self.wake {
            RenderWake::Idle => None,
            RenderWake::NextFrame => Some(now),
            RenderWake::At(at) => Some(now + at.saturating_duration_since(self.present_time())),
        }
    }

    fn startup_step(&mut self) -> bool {
        match &mut self.stage {
            Stage::Running(target) if target.startup_pending() => {
                target.startup_step();
                true
            }
            _ => false,
        }
    }

    fn idle_trim_deadline(&mut self, now: WallInstant) -> Option<WallInstant> {
        match &mut self.stage {
            Stage::Running(target) => target.idle_trim_in().map(|after| now + after),
            _ => None,
        }
    }

    fn tick(&mut self, now: WallInstant) -> Status {
        let closed = drain(&mut self.commands, &mut self.inbox);
        if closed && self.inbox.is_empty() {
            return Status::Closed;
        }
        let arrivals = self.stats.arrivals.load(Ordering::Acquire);
        let arrived = arrivals != self.seen_arrivals;
        let trim = self.stats.trim.load(Ordering::Acquire);
        let visibility = self.stats.hidden.load(Ordering::Acquire) != self.hidden_seen;
        let repaint_at = match self.pending {
            Pending::Settled => None,
            Pending::Continuous => Some(now),
            Pending::At(at) => Some(at),
        };
        let wake_at = self.wake_deadline(now);
        let idle_at = self.idle_trim_deadline(now);
        let idle_due = idle_at.is_some_and(|at| at <= now);
        if idle_due {
            self.stats.trim.store(true, Ordering::Release);
        }
        let trim = trim || idle_due;
        let due = |at: Option<WallInstant>| at.is_some_and(|at| at <= now);
        let urgent = !self.inbox.is_empty()
            || !self.feedback.is_empty()
            || arrived
            || trim
            || visibility
            || due(repaint_at)
            || due(wake_at);
        if !urgent && self.startup_step() {
            return Status::Ran;
        }
        if !urgent {
            let earliest = [repaint_at, wake_at, idle_at].into_iter().flatten().min();
            return Status::Waiting(earliest);
        }
        if let Some(not_before) = self.not_before.filter(|at| *at > now) {
            return Status::Waiting(Some(not_before));
        }
        self.cause = super::runs::RunCause {
            arrived,
            forced: false,
            trim,
            repaint: due(repaint_at),
            wake_due: due(wake_at),
            inbox: !self.inbox.is_empty(),
            feedback: !self.feedback.is_empty(),
            visibility,
            framed: false,
            presented: false,
        };
        self.run(now, arrived, trim, due(repaint_at));
        Status::Ran
    }

    fn run(&mut self, started: WallInstant, arrived: bool, trim: bool, repaint: bool) {
        crate::profile::begin_frame();
        self.stats.waits.fetch_add(1, Ordering::Relaxed);
        self.stats.picked_ns.store(epoch_ns(), Ordering::Relaxed);
        self.stats
            .service_thread
            .store(thread_token(), Ordering::Relaxed);
        if self.start() {
            let hidden = self.stats.hidden.load(Ordering::Acquire);
            if let Stage::Running(target) = &mut self.stage {
                if hidden != self.hidden_seen {
                    target.visibility(hidden);
                }
            }
            self.hidden_seen = hidden;
            if let Stage::Running(target) = &mut self.stage {
                let _span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Wait);
                target.wait_frame();
            }
        }
        self.stats.waited_ns.store(epoch_ns(), Ordering::Relaxed);
        let mut commands = std::mem::take(&mut self.inbox);
        drain(&mut self.commands, &mut commands);
        let scenes = commands
            .iter()
            .filter(|command| matches!(command, RenderCommand::Scene(_)))
            .count() as u64;
        let applied = self
            .stats
            .scenes_applied
            .fetch_add(scenes, Ordering::Relaxed)
            + scenes;
        if !commands.is_empty() {
            self.polls = 0;
        }
        self.seen_arrivals = self.stats.arrivals.load(Ordering::Acquire);
        let input = RenderInput {
            commands,
            feedback: std::mem::take(&mut self.feedback),
            present_at: self.present_time(),
        };
        let step_span = aqua_protocol::probe::span(aqua_protocol::probe::Phase::Input);
        let output = self.side.step(input);
        drop(step_span);
        self.stats.stepped_ns.store(epoch_ns(), Ordering::Relaxed);
        self.wake = output.wake;
        for event in output.events {
            if self.events.send(event).is_err() {
                break;
            }
        }
        let sized = self.side.viewport().is_some();
        let forced = output.frame.is_none() && (arrived || trim || repaint) && sized;
        let frame = match (output.frame.filter(|_| sized), forced) {
            (Some(frame), _) => {
                self.last_frame = frame;
                Some(frame)
            }
            (None, true) => Some(self.last_frame),
            (None, false) => None,
        };
        self.presented_last = false;
        if let Some(frame) = frame {
            if forced {
                self.stats.forced.fetch_add(1, Ordering::Relaxed);
            }
            let silent = forced && !trim && !arrived && repaint && self.quiet;
            let feedback = self.execute(frame, arrived || (forced && !trim && !silent));
            let maintenance = forced && trim;
            self.presented_last = maintenance
                || feedback
                    .iter()
                    .any(|item| matches!(item, Feedback::Presented(_)));
            self.feedback = match maintenance || silent {
                true => feedback
                    .into_iter()
                    .filter(|item| !matches!(item, Feedback::Presented(_)))
                    .collect(),
                false => feedback,
            };
            self.settle_pending(started);
            if self.presented_last {
                self.stats.presented_ns.store(epoch_ns(), Ordering::Relaxed);
                self.stats
                    .scenes_presented
                    .store(applied, Ordering::Release);
            }
        }
        if trim {
            self.stats.trim.store(false, Ordering::Release);
            crate::mem_probe::return_memory_to_os();
            if crate::mem_probe::memory_report_enabled() {
                eprintln!(
                    "aqua-trim: window {} trim pass {:.2} ms",
                    self.id,
                    started.elapsed().as_secs_f64() * 1e3
                );
            }
        }
        crate::profile::note(|| {
            format!(
                "render_cause=arrived:{arrived},forced:{forced},trim:{trim},repaint:{repaint},presented:{}",
                self.presented_last
            )
        });
        let totals = [
            self.stats.full_frames.load(Ordering::Relaxed),
            self.stats.partial_frames.load(Ordering::Relaxed),
            self.stats.culled_commands.load(Ordering::Relaxed),
        ];
        super::runs::record(
            super::runs::RunCause {
                forced,
                framed: frame.is_some(),
                presented: self.presented_last,
                ..self.cause
            },
            totals,
        );
        crate::profile::end_frame(crate::profile::Role::Render, self.id);
        self.not_before = match self.paced || !self.presented_last {
            true => Some(started + self.interval),
            false => None,
        };
    }

    fn settle_pending(&mut self, now: WallInstant) {
        let repaint = match &self.stage {
            Stage::Running(target) => target.repaint(),
            _ => Repaint::Settled,
        };
        self.pending = match repaint {
            Repaint::Settled => {
                self.quiet = false;
                self.polls = 0;
                Pending::Settled
            }
            Repaint::NextFrame => {
                self.quiet = false;
                Pending::Continuous
            }
            Repaint::Animating => {
                self.quiet = true;
                Pending::At(now + ANIMATION_PERIOD)
            }
            Repaint::Heavy => {
                self.quiet = true;
                Pending::At(now + HEAVY_ANIMATION_PERIOD)
            }
            Repaint::Still(rest) => {
                self.quiet = true;
                self.polls = 0;
                Pending::At(now + rest)
            }
            Repaint::Poll(delay) => match self.polls < MAX_PENDING_POLLS {
                true => {
                    self.polls += 1;
                    Pending::At(now + delay)
                }
                false => Pending::Settled,
            },
        };
    }

    fn execute(&mut self, frame: FrameId, invalidate: bool) -> Vec<Feedback> {
        let live = self.start();
        match (&mut self.stage, live) {
            (Stage::Running(target), true) => {
                if invalidate {
                    target.invalidate();
                }
                target.present(&mut self.side, frame)
            }
            (Stage::Dead { reported }, _) => match std::mem::replace(reported, true) {
                true => Vec::new(),
                false => vec![Feedback::DeviceLost],
            },
            _ => Vec::new(),
        }
    }

    fn finish(self) -> RenderSide {
        let Self { side, stage, .. } = self;
        drop(stage);
        side
    }
}

fn serve(shared: &Shared) {
    crate::native::raise_thread_priority();
    let mut slots: Vec<Slot> = Vec::new();
    loop {
        let mut shutdown = false;
        let controls: Vec<Control> = locked(&shared.control).drain(..).collect();
        for control in controls {
            match control {
                Control::Add(id, new) => slots.push(Slot::new(id, *new)),
                Control::Remove(id, reply) => {
                    let removed = slots
                        .iter()
                        .position(|slot| slot.id == id)
                        .map(|index| slots.remove(index).finish());
                    let _ = reply.send(removed);
                }
                Control::Shutdown => shutdown = true,
            }
        }
        if shutdown {
            break;
        }
        let now = WallInstant::now();
        let mut next: Option<WallInstant> = None;
        slots.retain_mut(|slot| match slot.tick(now) {
            Status::Closed => false,
            Status::Ran => {
                next = Some(now);
                true
            }
            Status::Waiting(at) => {
                next = [next, at].into_iter().flatten().min();
                true
            }
        });
        match next {
            Some(at) if at <= now => {}
            Some(at) => shared.signal.wait(Some(at - now)),
            None => shared.signal.wait(None),
        }
    }
}
