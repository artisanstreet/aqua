#![cfg(windows)]

use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_ime_tsf::TsfIme;
use aqua_protocol::{
    ImeBackend, ImeCommand, ImeEvent, Instant, Machine, PixelSize, RuntimeInput, RuntimeOutput,
    ShellEvent, WindowId,
};
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::style::{px, Style};
use std::cell::RefCell;
use std::rc::Rc;
use windows::core::{implement, Interface, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfContext, ITfEditSession, ITfEditSession_Impl, ITfRange, ITfRangeACP,
    ITfThreadMgr, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START, TF_DEFAULT_SELECTION,
    TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, ShowWindow, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

type SessionBody = Box<dyn FnOnce(u32) -> Result<()>>;

#[implement(ITfEditSession)]
struct Session {
    body: RefCell<Option<SessionBody>>,
}

impl ITfEditSession_Impl for Session_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        match self.body.borrow_mut().take() {
            Some(body) => body(cookie),
            None => Ok(()),
        }
    }
}

fn window_id() -> WindowId {
    WindowId::new(1).unwrap()
}

struct Rig {
    hwnd: HWND,
    runtime: Runtime,
    ime: TsfIme,
    thread_mgr: ITfThreadMgr,
    client: u32,
    now: f64,
    text: String,
    log: Vec<ImeEvent>,
}

impl Rig {
    fn new(initial: &str) -> Self {
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                windows::core::w!("STATIC"),
                windows::core::w!("aqua tsf runtime"),
                WS_POPUP,
                200,
                200,
                500,
                300,
                None,
                None,
                None,
                None,
            )
        }
        .expect("create window");
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
        let initial = initial.to_string();
        runtime
            .mount(move |_| {
                UiBox::new()
                    .child(
                        UiText::builder()
                            .arg(initial.as_str())
                            .editable(true)
                            .style(Style::new().width(px(300.0)).height(px(32.0)))
                            .build(),
                    )
                    .build()
            })
            .unwrap();
        let ime = TsfIme::from_hwnd(hwnd.0 as isize);
        let thread_mgr: ITfThreadMgr =
            unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let client = unsafe { thread_mgr.Activate() }.unwrap();
        let mut rig = Self {
            hwnd,
            runtime,
            ime,
            thread_mgr,
            client,
            now: 0.0,
            text: String::new(),
            log: Vec::new(),
        };
        rig.step(RuntimeInput::Shell(ShellEvent::Resized {
            window: window_id(),
            size: PixelSize {
                width: 400,
                height: 300,
            },
            scale: 1.0,
        }));
        rig.frame();
        rig.step(RuntimeInput::Shell(ShellEvent::Input {
            window: window_id(),
            event: InputEvent::PointerDown(
                PointerEvent {
                    contact: 0,
                    kind: PointerKind::Mouse,
                    position: Point::new(4.0, 10.0),
                    buttons: 1,
                },
                PointerButton::Primary,
            ),
        }));
        rig.frame();
        rig
    }

    fn step(&mut self, input: RuntimeInput) {
        let output = self.runtime.step(input);
        self.apply(output);
    }

    fn frame(&mut self) {
        self.now += 0.016;
        self.step(RuntimeInput::Shell(ShellEvent::Frame {
            window: window_id(),
            presentation: Instant::from_secs_f64(self.now),
        }));
    }

    fn apply(&mut self, output: RuntimeOutput) {
        for command in output.ime {
            if let ImeCommand::Enable(document) | ImeCommand::Update(document) = &command {
                self.text = document.text.clone();
            }
            self.ime.command(command).expect("tsf accepts command");
        }
    }

    fn settle(&mut self) {
        for _ in 0..4 {
            let mut events = Vec::new();
            self.ime.drain(&mut events);
            self.log.extend(events.iter().cloned());
            events
                .into_iter()
                .for_each(|event| self.step(RuntimeInput::Ime(event)));
            self.frame();
        }
    }

    #[track_caller]
    fn edit<T: 'static>(&self, body: impl FnOnce(&ITfContext, u32) -> Result<T> + 'static) -> T {
        let document = unsafe { self.thread_mgr.GetFocus() }.expect("focused document");
        let context = unsafe { document.GetTop() }.expect("top context");
        let slot = Rc::new(RefCell::new(None));
        let out = slot.clone();
        let inner = context.clone();
        let session: ITfEditSession = Session {
            body: RefCell::new(Some(Box::new(move |cookie| {
                *out.borrow_mut() = Some(body(&inner, cookie)?);
                Ok(())
            }))),
        }
        .into();
        let hr = unsafe {
            context.RequestEditSession(self.client, &session, TF_ES_READWRITE | TF_ES_SYNC)
        }
        .expect("request edit session");
        assert!(
            hr.is_ok(),
            "edit session {hr:?} at {}",
            std::panic::Location::caller()
        );
        let result = slot.borrow_mut().take().expect("session ran");
        result
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.ime.command(ImeCommand::Disable);
        let _ = unsafe { self.thread_mgr.Deactivate() };
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

fn range_at(context: &ITfContext, cookie: u32, start: i32, len: i32) -> Result<ITfRange> {
    let range = unsafe { context.GetStart(cookie) }?;
    let mut shifted = 0;
    unsafe { range.ShiftStart(cookie, start, &mut shifted, std::ptr::null()) }?;
    unsafe { range.Collapse(cookie, TF_ANCHOR_START) }?;
    unsafe { range.ShiftEnd(cookie, len, &mut shifted, std::ptr::null()) }?;
    Ok(range)
}

fn extent(range: &ITfRange) -> (i32, i32) {
    let acp: ITfRangeACP = range.cast().unwrap();
    let (mut start, mut len) = (0, 0);
    unsafe { acp.GetExtent(&mut start, &mut len) }.unwrap();
    (start, len)
}

#[test]
fn tsf_select_moves_the_editor_selection_and_the_next_frame_reports_it_back() {
    let mut rig = Rig::new("hello world");
    assert_eq!(rig.text, "hello world");
    rig.edit(|context, cookie| {
        let range = range_at(context, cookie, 6, 5)?;
        let selection = TF_SELECTION {
            range: std::mem::ManuallyDrop::new(Some(range)),
            style: TF_SELECTIONSTYLE {
                ase: TF_AE_NONE,
                fInterimChar: false.into(),
            },
        };
        unsafe { context.SetSelection(cookie, &[selection]) }
    });
    rig.settle();
    assert!(rig.log.iter().any(|event| matches!(
        event,
        ImeEvent::Select { range, .. } if (range.start, range.end) == (6, 11)
    )));
    assert_eq!(rig.runtime.selected_text().as_deref(), Some("world"));
    let seen = rig.edit(|context, cookie| {
        let mut selection = [TF_SELECTION::default()];
        let mut fetched = 0;
        unsafe {
            context.GetSelection(cookie, TF_DEFAULT_SELECTION, &mut selection, &mut fetched)
        }?;
        let range = selection[0].range.take().unwrap();
        Ok(extent(&range))
    });
    assert_eq!(seen, (6, 5));
}

#[test]
fn tsf_range_deletion_arrives_as_replace_and_removes_the_text_in_the_editor() {
    let mut rig = Rig::new("hello world");
    rig.edit(|context, cookie| {
        let range = range_at(context, cookie, 5, 6)?;
        unsafe { range.SetText(cookie, 0, &[]) }
    });
    rig.settle();
    assert!(
        rig.log.iter().any(|event| matches!(
            event,
            ImeEvent::Replace { range, text } if (range.start, range.end) == (5, 11) && text.is_empty()
        )),
        "events: {:?}",
        rig.log
    );
    assert_eq!(rig.text, "hello");
    let length = rig.edit(|context, cookie| {
        let start = unsafe { context.GetStart(cookie) }?;
        let end = unsafe { context.GetEnd(cookie) }?;
        unsafe { start.ShiftEndToRange(cookie, &end, TF_ANCHOR_END) }?;
        Ok(extent(&start).1)
    });
    assert_eq!(length, 5);
}
