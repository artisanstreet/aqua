#![cfg(target_os = "linux")]

use aqua_core::{ImeEvent as Composition, Rect};
use aqua_ime_wayland::{WaylandIme, WaylandImeError};
use aqua_protocol::{
    ImeBackend, ImeCommand, ImeDocument, ImeEvent, ImeGeometry, Instant, Utf16Range,
};
use aqua_shell::AppId;
use aqua_shell_wayland::testing::{
    capture, Compositor, Edit, InputMethod, Observed, VirtualKeyboard, POPUP_BGRA,
};
use aqua_shell_wayland::{now, quit, run, Handler, ShellEvent, Window, WindowOptions};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const TICK: Duration = Duration::from_millis(10);
const LIMIT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Field {
    text: String,
    caret: u32,
    marked: Option<String>,
    log: Vec<ImeEvent>,
}

fn byte(text: &str, units: u32) -> usize {
    let mut counted = 0;
    text.char_indices()
        .find(|(_, ch)| {
            let reached = counted >= units;
            counted += ch.len_utf16() as u32;
            reached
        })
        .map_or(text.len(), |(index, _)| index)
}

impl Field {
    fn insert(&mut self, range: Utf16Range, text: &str) {
        let (start, end) = (byte(&self.text, range.start), byte(&self.text, range.end));
        self.text.replace_range(start..end, text);
        self.caret = range.start + text.encode_utf16().count() as u32;
    }

    fn apply(&mut self, event: ImeEvent) {
        match &event {
            ImeEvent::Composition(Composition::Start) => self.marked = Some(String::new()),
            ImeEvent::Composition(Composition::Update { marked, .. }) => {
                self.marked = Some(marked.clone())
            }
            ImeEvent::Composition(Composition::Commit { text }) => {
                self.marked = None;
                self.insert(Utf16Range::new(self.caret, self.caret), text);
            }
            ImeEvent::Composition(Composition::Cancel) => self.marked = None,
            ImeEvent::Replace { range, text } => self.insert(*range, text),
            ImeEvent::Select { range, .. } => self.caret = range.end,
        }
        self.log.push(event);
    }

    fn document(&self) -> ImeDocument {
        ImeDocument {
            window_start: 0,
            total_len: self.text.encode_utf16().count() as u32,
            text: self.text.clone(),
            selection: Utf16Range::new(self.caret, self.caret),
            selection_reversed: false,
            area: Rect {
                x: 40.0 + 10.0 * self.caret as f32,
                y: 60.0,
                w: 2.0,
                h: 18.0,
            },
            geometry: ImeGeometry::UNAVAILABLE,
        }
    }

    fn commits(&self) -> usize {
        self.log
            .iter()
            .filter(|event| matches!(event, ImeEvent::Composition(Composition::Commit { .. })))
            .count()
    }
}

struct Editor {
    ime: WaylandIme,
    field: Field,
}

impl Editor {
    fn pump(&mut self) {
        let mut events = Vec::new();
        self.ime.drain(&mut events);
        let changed = !events.is_empty();
        events.into_iter().for_each(|event| self.field.apply(event));
        if changed {
            self.ime
                .command(ImeCommand::Update(self.field.document()))
                .unwrap();
        }
    }
}

struct Rig {
    compositor: Compositor,
    window: Window,
    method: InputMethod,
    editor: Editor,
    _keyboard: VirtualKeyboard,
}

fn rig(app: &str, scale: Option<&str>) -> Option<Rig> {
    let compositor = Compositor::start()?;
    compositor
        .attach()
        .expect("the shell connects to the headless compositor");
    if let Some(scale) = scale {
        compositor
            .command(&format!("output HEADLESS-1 scale {scale}"))
            .expect("sway accepts the scale");
    }
    let keyboard = VirtualKeyboard::with_layout(&compositor, "us", "").unwrap();
    let method = InputMethod::new(&compositor).unwrap();
    let window = Window::create(&WindowOptions {
        title: app.into(),
        logical_size: (480.0, 320.0),
        app_id: Some(AppId::new(app).unwrap()),
        ..WindowOptions::default()
    })
    .unwrap();
    let mut ime = WaylandIme::new(window).unwrap();
    let field = Field::default();
    ime.command(ImeCommand::Enable(field.document())).unwrap();
    Some(Rig {
        compositor,
        window,
        method,
        editor: Editor { ime, field },
        _keyboard: keyboard,
    })
}

enum Flow {
    Wait,
    Done,
}

struct Driver<F> {
    window: Window,
    step: F,
    limit: Instant,
    timed_out: Rc<RefCell<bool>>,
}

impl<F: FnMut() -> Flow + 'static> Handler for Driver<F> {
    fn window_event(&mut self, id: aqua_protocol::WindowId, event: ShellEvent) {
        if event == ShellEvent::Redraw && id == self.window.id() && self.window.is_visible() {
            let (width, height) = self.window.inner_size();
            let pixels = vec![0xc0u8; (width * height * 4) as usize];
            self.window.present_bgra(width, height, &pixels);
        }
    }

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        if at >= self.limit {
            *self.timed_out.borrow_mut() = true;
            quit();
            return None;
        }
        match (self.step)() {
            Flow::Wait => Some(at + TICK),
            Flow::Done => {
                quit();
                None
            }
        }
    }
}

fn drive(rig: Rig, mut step: impl FnMut(&mut Rig, &Observed) -> Flow + 'static) -> Rig {
    let window = rig.window;
    let shared = Rc::new(RefCell::new(rig));
    let inner = shared.clone();
    let timed_out = Rc::new(RefCell::new(false));
    run(Driver {
        window,
        step: move || {
            let mut rig = inner.borrow_mut();
            let observed = rig.method.poll().unwrap().clone();
            rig.editor.pump();
            step(&mut rig, &observed)
        },
        limit: now() + LIMIT,
        timed_out: timed_out.clone(),
    })
    .expect("the loop runs until the script quits");
    let rig = Rc::try_unwrap(shared)
        .ok()
        .expect("the driver released the rig")
        .into_inner();
    assert!(
        !*timed_out.borrow(),
        "the script did not finish within {LIMIT:?}; field {:?} {:?}, input method {:?}",
        rig.editor.field.text,
        rig.editor.field.log,
        rig.method.observed()
    );
    rig
}

fn send(rig: &mut Rig, edit: Edit<'_>) {
    rig.method.send(edit).unwrap();
}

#[test]
fn a_preedit_then_commit_from_the_input_method_inserts_the_committed_text_once() {
    let Some(rig) = rig("com.aqua.test.ime.commit", None) else {
        return;
    };
    let mut phase = 0;
    let rig = drive(rig, move |rig, observed| {
        let field = &rig.editor.field;
        phase = match phase {
            0 if observed.active => {
                send(
                    rig,
                    Edit {
                        preedit: Some(("にほん", 9, 9)),
                        ..Edit::default()
                    },
                );
                1
            }
            1 if field.marked.as_deref() == Some("にほん") => {
                send(
                    rig,
                    Edit {
                        commit: Some("日本"),
                        ..Edit::default()
                    },
                );
                2
            }
            2 if observed.surrounding == Some(("日本".into(), 6, 6)) => return Flow::Done,
            phase => phase,
        };
        Flow::Wait
    });
    let field = &rig.editor.field;
    assert_eq!(
        (field.text.as_str(), field.marked.as_deref()),
        ("日本", None)
    );
    assert_eq!(
        field.log,
        vec![
            ImeEvent::Composition(Composition::Start),
            ImeEvent::Composition(Composition::Update {
                marked: "にほん".into(),
                cursor: Some((9, 9)),
            }),
            ImeEvent::Composition(Composition::Commit {
                text: "日本".into()
            }),
        ]
    );
}

fn popup_origin(compositor: &Compositor, scale: u32) -> Option<(i32, i32)> {
    let shot = capture(compositor, false).ok()?;
    (0..shot.height)
        .flat_map(|y| (0..shot.width).map(move |x| (x, y)))
        .find(|(x, y)| {
            shot.bgra_at(*x, *y)
                .is_some_and(|pixel| pixel[..3] == POPUP_BGRA)
        })
        .map(|(x, y)| ((x / scale) as i32, (y / scale) as i32))
}

#[test]
fn the_candidate_popup_follows_the_caret_in_logical_pixels() {
    const APP: &str = "com.aqua.test.ime.caret";
    let Some(rig) = rig(APP, Some("2")) else {
        return;
    };
    let mut phase = 0;
    let origins = Rc::new(RefCell::new(Vec::new()));
    let seen = origins.clone();
    drive(rig, move |rig, observed| {
        let Some(view) = rig.compositor.view_rect(APP) else {
            return Flow::Wait;
        };
        let origin = observed
            .rectangle
            .and_then(|_| popup_origin(&rig.compositor, 2))
            .map(|(x, y)| (x - view.x, y - view.y));
        phase = match (phase, origin) {
            (0, Some(origin)) if origin == (40, 78) => {
                seen.borrow_mut().push((origin, observed.rectangle));
                send(
                    rig,
                    Edit {
                        commit: Some("ab"),
                        ..Edit::default()
                    },
                );
                1
            }
            (1, Some(origin)) if origin == (60, 78) => {
                seen.borrow_mut().push((origin, observed.rectangle));
                return Flow::Done;
            }
            (phase, _) => phase,
        };
        Flow::Wait
    });
    let origins = origins.take();
    assert_eq!(origins.len(), 2);
    for (_, rectangle) in origins {
        let (_, _, width, height) = rectangle.expect("the popup learned the caret rectangle");
        assert_eq!((width, height), (2, 18));
    }
}

fn echoing_reset(app: &str, command: ImeCommand, then: &'static str) -> Option<(Field, Observed)> {
    let rig = rig(app, None)?;
    let mut phase = 0;
    let rig = drive(rig, move |rig, observed| {
        let field = &rig.editor.field;
        phase = match phase {
            0 if observed.active => {
                send(
                    rig,
                    Edit {
                        preedit: Some(("にほん", 9, 9)),
                        ..Edit::default()
                    },
                );
                1
            }
            1 if field.marked.as_deref() == Some("にほん") => {
                rig.editor.ime.command(command.clone()).unwrap();
                rig.editor.pump();
                2
            }
            2 if observed.deactivations >= 1 && observed.active => {
                send(
                    rig,
                    Edit {
                        commit: Some("にほん"),
                        ..Edit::default()
                    },
                );
                3
            }
            3 => {
                send(
                    rig,
                    Edit {
                        commit: Some(then),
                        ..Edit::default()
                    },
                );
                4
            }
            4 if field.text.ends_with(then) => return Flow::Done,
            phase => phase,
        };
        Flow::Wait
    });
    Some((rig.editor.field, rig.method.observed().clone()))
}

#[test]
fn cancel_composition_discards_the_preedit_and_ignores_a_late_commit() {
    let Some((field, observed)) = echoing_reset(
        "com.aqua.test.ime.cancel",
        ImeCommand::CancelComposition,
        "x",
    ) else {
        return;
    };
    assert_eq!(field.text, "x");
    assert!(field
        .log
        .contains(&ImeEvent::Composition(Composition::Cancel)));
    assert_eq!(field.commits(), 0);
    assert!(observed.activations >= 2, "{observed:?}");
}

#[test]
fn commit_composition_inserts_the_preedit_once_when_the_input_method_echoes_it() {
    let Some((field, _)) = echoing_reset(
        "com.aqua.test.ime.commitnow",
        ImeCommand::CommitComposition,
        "!",
    ) else {
        return;
    };
    assert_eq!(field.text, "にほん!");
    assert_eq!(field.commits(), 1);
}

#[test]
fn reconversion_reports_unsupported() {
    let Some(rig) = rig("com.aqua.test.ime.reconvert", None) else {
        return;
    };
    let rig = drive(rig, |_, observed| match observed.active {
        true => Flow::Done,
        false => Flow::Wait,
    });
    let mut ime = rig.editor.ime;
    let refused = ime.command(ImeCommand::Reconvert);
    assert_eq!(refused, Err(WaylandImeError::Unsupported("reconversion")));
    assert!(refused.unwrap_err().to_string().contains("Wayland"));
}

#[test]
fn a_hidden_then_shown_window_composes_again() {
    const APP: &str = "com.aqua.test.ime.hide";
    let Some(rig) = rig(APP, None) else {
        return;
    };
    let mut phase = 0;
    let rig = drive(rig, move |rig, observed| {
        phase = match phase {
            0 if observed.active && rig.compositor.view_rect(APP).is_some() => {
                rig.window.hide();
                1
            }
            1 if rig.compositor.view_rect(APP).is_none() => {
                rig.window.show().unwrap();
                2
            }
            2 if observed.active && rig.compositor.view_rect(APP).is_some() => {
                send(
                    rig,
                    Edit {
                        commit: Some("ok"),
                        ..Edit::default()
                    },
                );
                3
            }
            3 if rig.editor.field.text == "ok" => return Flow::Done,
            phase => phase,
        };
        Flow::Wait
    });
    assert_eq!(rig.editor.field.text, "ok");
    drop(rig.compositor);
}
