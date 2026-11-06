#![cfg(windows)]

use aqua_core::{ImeEvent as Composition, Rect};
use aqua_ime_tsf::TsfIme;
use aqua_protocol::{ImeBackend, ImeCommand, ImeDocument, ImeEvent, Utf16Range};
use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::rc::Rc;
use windows::core::{implement, ComObject, Interface, Ref, Result, BOOL};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
    ITfContextComposition, ITfEditSession, ITfEditSession_Impl, ITfRange, ITfTextInputProcessor,
    ITfTextInputProcessor_Impl, ITfThreadMgr, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START,
    TF_DEFAULT_SELECTION, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, ShowWindow, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

const AREA: Rect = Rect {
    x: 30.0,
    y: 40.0,
    w: 3.0,
    h: 18.0,
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

#[implement(ITfTextInputProcessor, ITfCompositionSink)]
#[derive(Default)]
struct Service {
    thread_mgr: RefCell<Option<ITfThreadMgr>>,
    client: Cell<u32>,
    composition: RefCell<Option<ITfComposition>>,
    terminated: Cell<u32>,
    active: Cell<bool>,
}

impl ITfTextInputProcessor_Impl for Service_Impl {
    fn Activate(&self, thread_mgr: Ref<ITfThreadMgr>, client: u32) -> Result<()> {
        *self.thread_mgr.borrow_mut() = Some(thread_mgr.ok()?.clone());
        self.client.set(client);
        self.active.set(true);
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        self.thread_mgr.borrow_mut().take();
        self.composition.borrow_mut().take();
        self.active.set(false);
        Ok(())
    }
}

impl ITfCompositionSink_Impl for Service_Impl {
    fn OnCompositionTerminated(&self, _: u32, _: Ref<ITfComposition>) -> Result<()> {
        self.terminated.set(self.terminated.get() + 1);
        self.composition.borrow_mut().take();
        Ok(())
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

struct Rig {
    hwnd: HWND,
    ime: TsfIme,
    thread_mgr: ITfThreadMgr,
    client: u32,
    service: ComObject<Service>,
}

impl Rig {
    fn new(text: &str, selection: (u32, u32)) -> Rig {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                windows::core::w!("STATIC"),
                windows::core::w!("aqua-ime-tsf text service"),
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
        .expect("window");
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        let mut ime = TsfIme::from_hwnd(hwnd.0 as isize);
        ime.command(ImeCommand::Enable(ImeDocument {
            window_start: 0,
            total_len: text.encode_utf16().count() as u32,
            text: text.to_string(),
            selection: Utf16Range::new(selection.0, selection.1),
            selection_reversed: false,
            area: AREA,
        }))
        .expect("enable");
        let thread_mgr: ITfThreadMgr =
            unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let client = unsafe { thread_mgr.Activate() }.unwrap();
        let service = ComObject::new(Service::default());
        let processor: ITfTextInputProcessor = service.to_interface();
        unsafe { processor.Activate(&thread_mgr, client) }.unwrap();
        Rig {
            hwnd,
            ime,
            thread_mgr,
            client,
            service,
        }
    }

    fn edit<T: 'static>(&self, body: impl FnOnce(&ITfContext, u32) -> Result<T> + 'static) -> T {
        let document = unsafe { self.thread_mgr.GetFocus() }.expect("focused document");
        let context = unsafe { document.GetTop() }.expect("context");
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
        .expect("request");
        assert!(hr.is_ok(), "{hr:?}");
        let result = slot.borrow_mut().take().expect("session ran");
        result
    }

    fn begin(&self) {
        let service = self.service.clone();
        self.edit(move |context, cookie| {
            let mut selection = [TF_SELECTION::default()];
            let mut fetched = 0;
            unsafe {
                context.GetSelection(cookie, TF_DEFAULT_SELECTION, &mut selection, &mut fetched)
            }?;
            let range = selection[0]
                .range
                .take()
                .ok_or_else(windows::core::Error::empty)?;
            let api: ITfContextComposition = context.cast()?;
            let sink: ITfCompositionSink = service.to_interface();
            let composition = unsafe { api.StartComposition(cookie, &range, &sink) }?;
            *service.composition.borrow_mut() = Some(composition);
            Ok(())
        });
    }

    fn set_text(&self, text: &str, caret: Option<i32>) {
        let service = self.service.clone();
        let text = wide(text);
        self.edit(move |context, cookie| {
            let composition = service
                .composition
                .borrow()
                .clone()
                .ok_or_else(windows::core::Error::empty)?;
            let range = unsafe { composition.GetRange() }?;
            unsafe { range.SetText(cookie, 0, &text) }?;
            if let Some(caret) = caret {
                let at: ITfRange = unsafe { range.Clone() }?;
                unsafe { at.Collapse(cookie, TF_ANCHOR_START) }?;
                unsafe { at.ShiftEnd(cookie, caret, &mut 0, std::ptr::null()) }?;
                unsafe { at.Collapse(cookie, TF_ANCHOR_END) }?;
                let selection = [TF_SELECTION {
                    range: ManuallyDrop::new(Some(at)),
                    style: TF_SELECTIONSTYLE {
                        ase: TF_AE_NONE,
                        fInterimChar: BOOL(0),
                    },
                }];
                unsafe { context.SetSelection(cookie, &selection) }?;
            }
            Ok(())
        });
    }

    fn end(&self) {
        let service = self.service.clone();
        self.edit(move |_, cookie| {
            let composition = service.composition.borrow_mut().take();
            if let Some(composition) = composition {
                unsafe { composition.EndComposition(cookie) }?;
            }
            Ok(())
        });
    }

    fn events(&mut self) -> Vec<ImeEvent> {
        let mut out = Vec::new();
        self.ime.drain(&mut out);
        out
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let processor: ITfTextInputProcessor = self.service.to_interface();
        let _ = unsafe { processor.Deactivate() };
        let _ = self.ime.command(ImeCommand::Disable);
        let _ = unsafe { self.thread_mgr.Deactivate() };
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

fn update(marked: &str, cursor: (usize, usize)) -> ImeEvent {
    ImeEvent::Composition(Composition::Update {
        marked: marked.into(),
        cursor: Some(cursor),
    })
}

fn commit(text: &str) -> ImeEvent {
    ImeEvent::Composition(Composition::Commit { text: text.into() })
}

const START: ImeEvent = ImeEvent::Composition(Composition::Start);

#[test]
fn a_clause_conversion_session_reports_byte_cursors_inside_the_marked_text() {
    let mut rig = Rig::new("", (0, 0));
    assert!(rig.service.active.get());
    rig.begin();
    rig.set_text("\u{304B}\u{3093}\u{3058}", Some(3));
    rig.set_text("\u{6F22}\u{5B57}", Some(1));
    rig.end();
    let events = rig.events();
    assert_eq!(events.first(), Some(&START));
    assert!(
        events.contains(&update("\u{304B}\u{3093}\u{3058}", (9, 9))),
        "{events:?}"
    );
    assert!(
        events.contains(&update("\u{6F22}\u{5B57}", (3, 3))),
        "{events:?}"
    );
    assert_eq!(events.last(), Some(&commit("\u{6F22}\u{5B57}")));
}

#[test]
fn composition_after_astral_text_reports_utf8_cursors_for_utf16_positions() {
    let mut rig = Rig::new("a\u{1F600}", (3, 3));
    rig.begin();
    rig.set_text("\u{1F601}b", Some(2));
    rig.end();
    let events = rig.events();
    assert!(events.contains(&update("\u{1F601}b", (4, 4))), "{events:?}");
    assert_eq!(events.last(), Some(&commit("\u{1F601}b")));
}

#[test]
fn a_composition_over_a_selection_and_a_second_one_after_it_each_commit_once() {
    let mut rig = Rig::new("hello world", (6, 11));
    rig.begin();
    rig.set_text("sekai", None);
    rig.end();
    rig.begin();
    rig.set_text("!", None);
    rig.end();
    let events = rig.events();
    let commits: Vec<&ImeEvent> = events
        .iter()
        .filter(|event| matches!(event, ImeEvent::Composition(Composition::Commit { .. })))
        .collect();
    let starts = events.iter().filter(|event| **event == START).count();
    assert_eq!(commits, vec![&commit("sekai"), &commit("!")], "{events:?}");
    assert_eq!(starts, 2, "{events:?}");
}

#[test]
fn cancelling_from_the_app_ends_the_service_composition_without_a_commit() {
    let mut rig = Rig::new("ab", (2, 2));
    rig.begin();
    rig.set_text("kan", Some(3));
    rig.events();
    rig.ime.command(ImeCommand::CancelComposition).unwrap();
    let events = rig.events();
    assert_eq!(events, vec![ImeEvent::Composition(Composition::Cancel)]);
    assert!(!events
        .iter()
        .any(|event| matches!(event, ImeEvent::Composition(Composition::Commit { .. }))));
}

#[test]
fn committing_from_the_app_finalises_the_marked_text() {
    let mut rig = Rig::new("", (0, 0));
    rig.begin();
    rig.set_text("nihon", Some(5));
    rig.events();
    rig.ime.command(ImeCommand::CommitComposition).unwrap();
    let events = rig.events();
    assert_eq!(events.last(), Some(&commit("nihon")), "{events:?}");
}
