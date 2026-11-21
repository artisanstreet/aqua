use crate::{now, quit, run, Handler, ShellEvent};
use aqua_protocol::{Instant, WindowId};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

pub const POLL: Duration = Duration::from_millis(16);

pub type Entry = (WindowId, ShellEvent, Instant);

pub enum Flow {
    Wait(Duration),
    Done,
}

pub struct Context<'a> {
    pub log: &'a [Entry],
    pub since: usize,
    pub started: Instant,
    pub now: Instant,
}

impl Context<'_> {
    pub fn elapsed(&self) -> Duration {
        self.now.saturating_duration_since(self.started)
    }

    pub fn recent(&self) -> &[Entry] {
        &self.log[self.since.min(self.log.len())..]
    }

    pub fn saw(&self, wanted: impl Fn(&ShellEvent) -> bool) -> bool {
        self.recent().iter().any(|(_, event, _)| wanted(event))
    }

    pub fn saw_on(&self, id: WindowId, wanted: impl Fn(&ShellEvent) -> bool) -> bool {
        self.recent()
            .iter()
            .any(|(window, event, _)| *window == id && wanted(event))
    }

    pub fn count(&self, wanted: impl Fn(&ShellEvent) -> bool) -> usize {
        self.recent()
            .iter()
            .filter(|(_, event, _)| wanted(event))
            .count()
    }
}

struct Driver<F> {
    log: Rc<RefCell<Vec<Entry>>>,
    step: F,
    limit: Instant,
    timed_out: Rc<RefCell<bool>>,
}

impl<F: FnMut(&[Entry], Instant) -> Flow + 'static> Handler for Driver<F> {
    fn window_event(&mut self, id: WindowId, event: ShellEvent) {
        self.log.borrow_mut().push((id, event, now()));
    }

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        if at >= self.limit {
            *self.timed_out.borrow_mut() = true;
            quit();
            return None;
        }
        let log = self.log.borrow().clone();
        match (self.step)(&log, at) {
            Flow::Wait(delay) => Some(at + delay),
            Flow::Done => {
                quit();
                None
            }
        }
    }
}

pub fn drive(limit: Duration, step: impl FnMut(&[Entry], Instant) -> Flow + 'static) -> Vec<Entry> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let timed_out = Rc::new(RefCell::new(false));
    crate::schedule(now());
    run(Driver {
        log: log.clone(),
        step,
        limit: now() + limit,
        timed_out: timed_out.clone(),
    })
    .expect("the loop runs until the script quits");
    let events = log.borrow().clone();
    assert!(
        !*timed_out.borrow(),
        "the script did not finish within {limit:?}; events: {events:#?}"
    );
    events
}

type Action = Box<dyn FnMut()>;
type Until = Box<dyn Fn(&Context<'_>) -> bool>;

pub struct Stage {
    name: &'static str,
    act: Action,
    until: Until,
}

pub fn stage(
    name: &'static str,
    act: impl FnMut() + 'static,
    until: impl Fn(&Context<'_>) -> bool + 'static,
) -> Stage {
    Stage {
        name,
        act: Box::new(act),
        until: Box::new(until),
    }
}

pub fn script(limit: Duration, stages: Vec<Stage>) -> Vec<Entry> {
    let reached = Rc::new(RefCell::new(""));
    let progress = reached.clone();
    let mut stages = stages.into_iter();
    let mut current: Option<(Stage, usize, Instant)> = None;
    let step = move |log: &[Entry], at: Instant| loop {
        match current.as_mut() {
            None => match stages.next() {
                None => return Flow::Done,
                Some(mut next) => {
                    *progress.borrow_mut() = next.name;
                    (next.act)();
                    current = Some((next, log.len(), at));
                    return Flow::Wait(POLL);
                }
            },
            Some((stage, since, started)) => {
                let context = Context {
                    log,
                    since: *since,
                    started: *started,
                    now: at,
                };
                match (stage.until)(&context) {
                    true => current = None,
                    false => return Flow::Wait(POLL),
                }
            }
        }
    };
    let log = Rc::new(RefCell::new(Vec::new()));
    let timed_out = Rc::new(RefCell::new(false));
    crate::schedule(now());
    run(Driver {
        log: log.clone(),
        step,
        limit: now() + limit,
        timed_out: timed_out.clone(),
    })
    .expect("the loop runs until the script quits");
    let events = log.borrow().clone();
    assert!(
        !*timed_out.borrow(),
        "stage {:?} did not finish within {limit:?}; events: {events:#?}",
        reached.borrow()
    );
    events
}

pub fn events_of(log: &[Entry], id: WindowId) -> Vec<ShellEvent> {
    log.iter()
        .filter(|(window, _, _)| *window == id)
        .map(|(_, event, _)| event.clone())
        .collect()
}

pub fn inputs(log: &[Entry]) -> Vec<aqua_core::InputEvent> {
    log.iter()
        .filter_map(|(_, event, _)| match event {
            ShellEvent::Input(input) => Some(input.clone()),
            _ => None,
        })
        .collect()
}
