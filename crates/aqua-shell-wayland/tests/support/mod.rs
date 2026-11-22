#![allow(dead_code)]

use aqua_protocol::{Instant, WindowId};
use aqua_shell::AppId;
use aqua_shell_wayland::testing::{Compositor, Options};
use aqua_shell_wayland::{now, quit, run, Handler, ShellEvent, Window, WindowOptions};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

pub type Entry = (WindowId, ShellEvent, Instant);
pub type Log = Rc<RefCell<Vec<Entry>>>;

pub fn compositor() -> Option<Rc<Compositor>> {
    compositor_with(Options::default())
}

pub fn compositor_with(options: Options) -> Option<Rc<Compositor>> {
    let compositor = Compositor::start_with(options)?;
    compositor
        .attach()
        .expect("the shell connects to the headless compositor");
    Some(Rc::new(compositor))
}

pub fn floating(compositor: &Compositor, app_id: &str) {
    compositor
        .command(&format!(
            "for_window [app_id=\"^{app_id}$\"] floating enable"
        ))
        .expect("sway accepts the floating rule");
}

pub fn options(app_id: &str, logical: (f64, f64)) -> WindowOptions {
    WindowOptions {
        title: app_id.into(),
        logical_size: logical,
        app_id: Some(AppId::new(app_id).expect("test app ids are valid")),
        ..WindowOptions::default()
    }
}

pub enum Flow {
    Wait(Duration),
    Done,
}

pub struct Context<'a> {
    pub log: &'a [Entry],
    pub now: Instant,
}

impl Context<'_> {
    pub fn saw(&self, wanted: impl Fn(&ShellEvent) -> bool) -> bool {
        self.log.iter().any(|(_, event, _)| wanted(event))
    }

    pub fn count(&self, wanted: impl Fn(&ShellEvent) -> bool) -> usize {
        self.log
            .iter()
            .filter(|(_, event, _)| wanted(event))
            .count()
    }

    pub fn focused(&self) -> bool {
        self.saw(|event| *event == ShellEvent::Focused(true))
    }
}

struct Driver<F> {
    log: Log,
    windows: Vec<Window>,
    step: F,
    limit: Instant,
    timed_out: Rc<RefCell<bool>>,
}

pub fn paint(window: Window) {
    let (width, height) = window.inner_size();
    let pixels = vec![0x80u8; (width as usize) * (height as usize) * 4];
    window.present_bgra(width, height, &pixels);
}

impl<F: FnMut(&Context<'_>) -> Flow + 'static> Handler for Driver<F> {
    fn window_event(&mut self, id: WindowId, event: ShellEvent) {
        if event == ShellEvent::Redraw
            && let Some(window) = self.windows.iter().find(|window| window.id() == id)
            && window.is_visible()
        {
            paint(*window);
        }
        self.log.borrow_mut().push((id, event, now()));
    }

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        if at >= self.limit {
            *self.timed_out.borrow_mut() = true;
            quit();
            return None;
        }
        let log = self.log.borrow().clone();
        match (self.step)(&Context { log: &log, now: at }) {
            Flow::Wait(delay) => Some(at + delay),
            Flow::Done => {
                quit();
                None
            }
        }
    }
}

pub fn drive(
    windows: &[Window],
    limit: Duration,
    step: impl FnMut(&Context<'_>) -> Flow + 'static,
) -> Vec<Entry> {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let timed_out = Rc::new(RefCell::new(false));
    run(Driver {
        log: log.clone(),
        windows: windows.to_vec(),
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

pub fn events(log: &[Entry]) -> Vec<ShellEvent> {
    log.iter().map(|(_, event, _)| event.clone()).collect()
}

pub fn inputs(log: &[Entry]) -> Vec<aqua_core::InputEvent> {
    log.iter()
        .filter_map(|(_, event, _)| match event {
            ShellEvent::Input(input) => Some(input.clone()),
            _ => None,
        })
        .collect()
}

pub fn mapped(compositor: &Compositor, app_id: &str) -> Option<aqua_shell_wayland::testing::Rect> {
    compositor
        .view_rect(app_id)
        .filter(|rect| rect.width > 0 && rect.height > 0)
}

pub fn ready(compositor: &Compositor, app_id: &str) -> Option<aqua_shell_wayland::testing::Rect> {
    compositor
        .view(app_id)
        .filter(|view| view["focused"] == serde_json::Value::Bool(true))
        .and_then(|_| mapped(compositor, app_id))
}
