#![allow(dead_code)]

use aqua_protocol::{Instant, WindowId};
use aqua_shell_appkit::testing::{available, Facility};
use aqua_shell_appkit::{now, quit, run, Handler, ShellEvent, WindowOptions};
use std::cell::RefCell;
use std::io::Read;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

const CHILD: &str = "AQUA_APPKIT_CASE";
const CASE_LIMIT: Duration = Duration::from_secs(40);
pub const POLL: Duration = Duration::from_millis(16);

pub struct Case {
    pub name: &'static str,
    pub run: fn(),
    pub args: &'static [&'static str],
}

pub const fn case(name: &'static str, run: fn()) -> Case {
    Case {
        name,
        run,
        args: &[],
    }
}

pub const fn case_with(name: &'static str, run: fn(), args: &'static [&'static str]) -> Case {
    Case { name, run, args }
}

pub fn main(cases: &[Case]) {
    match std::env::var(CHILD) {
        Ok(name) => {
            let case = cases
                .iter()
                .find(|case| case.name == name)
                .unwrap_or_else(|| panic!("no case named {name}"));
            (case.run)();
        }
        Err(_) => parent(cases),
    }
}

enum Outcome {
    Passed(String),
    Skipped(String),
    Failed(String),
}

fn collect(mut stream: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        stream
            .read_to_string(&mut text)
            .expect("the child writes UTF-8 output");
        text
    })
}

fn spawn_case(case: &Case) -> Outcome {
    let exe = std::env::current_exe().expect("the test binary knows its path");
    let mut child = Command::new(exe)
        .args(case.args)
        .env(CHILD, case.name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the test binary can start itself");
    let stdout = collect(child.stdout.take().expect("stdout is piped"));
    let stderr = collect(child.stderr.take().expect("stderr is piped"));
    let pid = child.id();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let status = child.wait();
        let _ = sender.send(status);
    });
    let status = receiver.recv_timeout(CASE_LIMIT);
    if status.is_err() {
        let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
    }
    let output = format!(
        "{}{}",
        stdout.join().expect("the stdout reader finishes"),
        stderr.join().expect("the stderr reader finishes")
    );
    match status {
        Ok(Ok(status)) if status.success() => {
            match output.lines().find(|line| line.starts_with("SKIP-")) {
                Some(line) => Outcome::Skipped(line.to_owned()),
                None => Outcome::Passed(output),
            }
        }
        Ok(Ok(status)) => Outcome::Failed(format!("exited with {status}\n{output}")),
        Ok(Err(error)) => Outcome::Failed(format!("could not wait: {error}\n{output}")),
        Err(_) => Outcome::Failed(format!("timed out after {CASE_LIMIT:?}\n{output}")),
    }
}

fn parent(cases: &[Case]) {
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let selected: Vec<&Case> = cases
        .iter()
        .filter(|case| {
            filters.is_empty()
                || filters
                    .iter()
                    .any(|filter| case.name.contains(filter.as_str()))
        })
        .collect();
    println!("running {} tests", selected.len());
    let mut failures = Vec::new();
    let mut skipped = 0;
    for case in &selected {
        match spawn_case(case) {
            Outcome::Passed(output) => {
                println!("test {} ... ok", case.name);
                output
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .for_each(|line| println!("    {line}"));
            }
            Outcome::Skipped(reason) => {
                skipped += 1;
                println!("test {} ... skipped ({reason})", case.name);
            }
            Outcome::Failed(output) => {
                println!("test {} ... FAILED\n{output}", case.name);
                failures.push(case.name);
            }
        }
    }
    println!(
        "test result: {}. {} passed; {} failed; {skipped} skipped",
        if failures.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failures.len() - skipped,
        failures.len()
    );
    if !failures.is_empty() {
        println!("failures: {failures:?}");
        std::process::exit(1);
    }
}

pub fn appkit() -> bool {
    available(Facility::WindowServer)
}

pub fn posting() -> bool {
    appkit() && available(Facility::PostEvent)
}

pub fn options(title: &str, logical: (f64, f64)) -> WindowOptions {
    WindowOptions {
        title: title.into(),
        logical_size: logical,
        ..WindowOptions::default()
    }
}

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
    aqua_shell_appkit::schedule(now());
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
    aqua_shell_appkit::schedule(now());
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
