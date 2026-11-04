#![cfg(windows)]

use aqua_core::{ImeEvent as Composition, Rect};
use aqua_ime_tsf::TsfIme;
use aqua_protocol::{ImeBackend, ImeCommand, ImeDocument, ImeEvent, Utf16Range};
use std::cell::RefCell;
use std::rc::Rc;
use windows::core::{implement, Interface, Ref, Result};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
    ITfContextComposition, ITfEditSession, ITfEditSession_Impl, ITfInsertAtSelection, ITfRange,
    ITfRangeACP, ITfThreadMgr, INSERT_TEXT_AT_SELECTION_FLAGS, TF_ANCHOR_END, TF_DEFAULT_SELECTION,
    TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
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

#[implement(ITfCompositionSink)]
struct Sink;

impl ITfCompositionSink_Impl for Sink_Impl {
    fn OnCompositionTerminated(
        &self,
        _cookie: u32,
        _composition: Ref<ITfComposition>,
    ) -> Result<()> {
        Ok(())
    }
}

fn sink() -> ITfCompositionSink {
    Sink.into()
}

struct Harness {
    hwnd: HWND,
    ime: TsfIme,
    client: u32,
    thread_mgr: ITfThreadMgr,
}

fn document(text: &str, selection: (u32, u32)) -> ImeDocument {
    ImeDocument {
        window_start: 0,
        total_len: text.encode_utf16().count() as u32,
        text: text.to_string(),
        selection: Utf16Range::new(selection.0, selection.1),
        selection_reversed: false,
        area: AREA,
    }
}

impl Harness {
    fn new(doc: &ImeDocument) -> Self {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                windows::core::w!("STATIC"),
                windows::core::w!("aqua-ime-tsf test"),
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
        .expect("create test window");
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        let mut ime = TsfIme::from_hwnd(hwnd.0 as isize);
        ime.command(ImeCommand::Enable(doc.clone()))
            .expect("enable tsf");
        let thread_mgr: ITfThreadMgr =
            unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let client = unsafe { thread_mgr.Activate() }.unwrap();
        Self {
            hwnd,
            ime,
            client,
            thread_mgr,
        }
    }

    fn context(&self) -> ITfContext {
        let document = unsafe { self.thread_mgr.GetFocus() }.expect("a focused document");
        unsafe { document.GetTop() }.expect("top context")
    }

    fn edit<T: 'static>(&self, body: impl FnOnce(&ITfContext, u32) -> Result<T> + 'static) -> T {
        let context = self.context();
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
        assert!(hr.is_ok(), "session hr {hr:?}");
        let result = slot.borrow_mut().take().expect("session ran");
        result
    }

    fn events(&mut self) -> Vec<ImeEvent> {
        let mut out = Vec::new();
        self.ime.drain(&mut out);
        out
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.ime.command(ImeCommand::Disable);
        let _ = unsafe { self.thread_mgr.Deactivate() };
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

fn text_of(range: &ITfRange, cookie: u32) -> String {
    let mut buffer = [0u16; 128];
    let mut count = 0u32;
    unsafe { range.GetText(cookie, 0, &mut buffer, &mut count) }.unwrap();
    String::from_utf16_lossy(&buffer[..count as usize])
}

fn extent(range: &ITfRange) -> (i32, i32) {
    let acp: ITfRangeACP = range.cast().unwrap();
    let (mut start, mut len) = (0, 0);
    unsafe { acp.GetExtent(&mut start, &mut len) }.unwrap();
    (start, len)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn document_text_selection_and_utf16_ranges_are_visible_to_tsf() {
    let harness = Harness::new(&document("hello \u{1F600} world", (8, 8)));
    let (selection, whole, emoji) = harness.edit(|context, cookie| {
        let mut selection = [TF_SELECTION::default()];
        let mut fetched = 0;
        unsafe {
            context.GetSelection(cookie, TF_DEFAULT_SELECTION, &mut selection, &mut fetched)
        }?;
        let range = selection[0].range.take().unwrap();
        let start = unsafe { context.GetStart(cookie) }?;
        let end = unsafe { context.GetEnd(cookie) }?;
        unsafe { start.ShiftEndToRange(cookie, &end, TF_ANCHOR_END) }?;
        let emoji = unsafe { range.Clone() }?;
        let mut shifted = 0;
        unsafe { emoji.ShiftStart(cookie, -2, &mut shifted, std::ptr::null()) }?;
        Ok((
            extent(&range),
            (extent(&start), text_of(&start, cookie)),
            (shifted, text_of(&emoji, cookie)),
        ))
    });
    assert_eq!(selection, (8, 0));
    assert_eq!(whole, ((0, 14), "hello \u{1F600} world".to_string()));
    assert_eq!(emoji, (-2, "\u{1F600}".to_string()));
}

#[test]
fn candidate_window_position_follows_the_document_area() {
    let mut harness = Harness::new(&document("abc", (3, 3)));
    let expected = |area: Rect, hwnd: HWND| {
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
        let mut origin = POINT::default();
        let _ = unsafe { ClientToScreen(hwnd, &mut origin) };
        (
            origin.x + (area.x * dpi).round() as i32,
            origin.y + (area.y * dpi).round() as i32,
            ((area.h * dpi).round() as i32).max(1),
        )
    };
    let hwnd = harness.hwnd;
    let measure = |harness: &Harness| {
        harness.edit(|context, cookie| {
            let range = unsafe { context.GetEnd(cookie) }?;
            let view = unsafe { context.GetActiveView() }?;
            let mut rect = RECT::default();
            let mut clipped = Default::default();
            unsafe { view.GetTextExt(cookie, &range, &mut rect, &mut clipped) }?;
            Ok((rect.left, rect.top, rect.bottom - rect.top))
        })
    };
    assert_eq!(measure(&harness), expected(AREA, hwnd));
    let moved = Rect {
        x: 120.0,
        y: 90.0,
        w: 3.0,
        h: 24.0,
    };
    let mut doc = document("abc", (3, 3));
    doc.area = moved;
    harness.ime.command(ImeCommand::Update(doc)).unwrap();
    assert_eq!(measure(&harness), expected(moved, hwnd));
}

#[test]
fn composition_flows_through_tsf_into_events() {
    let mut harness = Harness::new(&document("ab", (2, 2)));
    harness.edit(|context, cookie| {
        let selection_range = {
            let mut selection = [TF_SELECTION::default()];
            let mut fetched = 0;
            unsafe {
                context.GetSelection(cookie, TF_DEFAULT_SELECTION, &mut selection, &mut fetched)
            }?;
            selection[0].range.take().unwrap()
        };
        let composition_api: ITfContextComposition = context.cast()?;
        let composition =
            unsafe { composition_api.StartComposition(cookie, &selection_range, &sink()) }
                .inspect_err(|e| eprintln!("StartComposition {e:?}"))?;
        let range =
            unsafe { composition.GetRange() }.inspect_err(|e| eprintln!("GetRange {e:?}"))?;
        let ka = wide("ka");
        unsafe { range.SetText(cookie, 0, &ka) }.inspect_err(|e| eprintln!("SetText1 {e:?}"))?;
        let kana = wide("\u{304B}");
        unsafe { range.SetText(cookie, 0, &kana) }.inspect_err(|e| eprintln!("SetText2 {e:?}"))?;
        unsafe { composition.EndComposition(cookie) }.inspect_err(|e| eprintln!("End {e:?}"))?;
        Ok(())
    });
    let events = harness.events();
    assert_eq!(
        events.first(),
        Some(&ImeEvent::Composition(Composition::Start))
    );
    assert!(events.contains(&ImeEvent::Composition(Composition::Update {
        marked: "ka".into(),
        cursor: Some((2, 2))
    })));
    assert!(events.contains(&ImeEvent::Composition(Composition::Update {
        marked: "\u{304B}".into(),
        cursor: Some((3, 3))
    })));
    assert_eq!(
        events.last(),
        Some(&ImeEvent::Composition(Composition::Commit {
            text: "\u{304B}".into()
        }))
    );
}

#[test]
fn direct_insertion_commits_and_range_edits_replace() {
    let mut harness = Harness::new(&document("hello world", (5, 5)));
    harness.edit(|context, cookie| {
        let insert: ITfInsertAtSelection = context.cast()?;
        let text = wide("!");
        unsafe { insert.InsertTextAtSelection(cookie, INSERT_TEXT_AT_SELECTION_FLAGS(0), &text) }?;
        let start = unsafe { context.GetStart(cookie) }?;
        let upper = wide("HELLO");
        unsafe { start.ShiftEnd(cookie, 5, &mut 0, std::ptr::null()) }?;
        unsafe { start.SetText(cookie, 0, &upper) }?;
        Ok(())
    });
    let events = harness.events();
    assert_eq!(
        events,
        vec![
            ImeEvent::Composition(Composition::Start),
            ImeEvent::Composition(Composition::Update {
                marked: "!".into(),
                cursor: Some((1, 1))
            }),
            ImeEvent::Composition(Composition::Commit { text: "!".into() }),
            ImeEvent::Select {
                range: Utf16Range::new(0, 5),
                reversed: false
            },
            ImeEvent::Composition(Composition::Start),
            ImeEvent::Composition(Composition::Update {
                marked: "HELLO".into(),
                cursor: Some((5, 5))
            }),
            ImeEvent::Composition(Composition::Commit {
                text: "HELLO".into()
            }),
        ]
    );
}

#[test]
fn app_updates_reach_tsf_and_cancel_discards_composition_text() {
    let mut harness = Harness::new(&document("abc", (3, 3)));
    harness
        .ime
        .command(ImeCommand::Update(document("abcdef", (6, 6))))
        .unwrap();
    let (end, selection) = harness.edit(|context, cookie| {
        let end = unsafe { context.GetEnd(cookie) }?;
        let mut selection = [TF_SELECTION::default()];
        let mut fetched = 0;
        unsafe {
            context.GetSelection(cookie, TF_DEFAULT_SELECTION, &mut selection, &mut fetched)
        }?;
        let range = selection[0].range.take().unwrap();
        Ok((extent(&end).0, extent(&range).0))
    });
    assert_eq!((end, selection), (6, 6));

    let context = harness.context();
    let client = harness.client;
    let compose: Rc<RefCell<Option<windows::Win32::UI::TextServices::ITfComposition>>> =
        Rc::default();
    let hold = compose.clone();
    harness.edit(move |context, cookie| {
        let start = unsafe { context.GetEnd(cookie) }?;
        let api: ITfContextComposition = context.cast()?;
        let composition = unsafe { api.StartComposition(cookie, &start, &sink()) }?;
        let range = unsafe { composition.GetRange() }?;
        let text = wide("zz");
        unsafe { range.SetText(cookie, 0, &text) }?;
        *hold.borrow_mut() = Some(composition);
        Ok(())
    });
    drop((context, client));
    harness.events();
    harness.ime.command(ImeCommand::CancelComposition).unwrap();
    assert_eq!(
        harness.events(),
        vec![ImeEvent::Composition(Composition::Cancel)]
    );
    let end = harness.edit(|context, cookie| Ok(extent(&unsafe { context.GetEnd(cookie) }?).0));
    assert_eq!(end, 6);
}

#[test]
fn reconversion_command_reports_without_a_text_service() {
    let mut harness = Harness::new(&document("\u{5909}\u{63DB}", (0, 2)));
    let outcome = harness.ime.command(ImeCommand::Reconvert);
    assert!(
        matches!(
            outcome,
            Ok(()) | Err(aqua_ime_tsf::TsfError::NotReconvertible)
        ),
        "{outcome:?}"
    );
}

#[test]
fn disable_removes_tsf_focus_and_enable_restores_it() {
    let mut harness = Harness::new(&document("x", (1, 1)));
    let ours = harness.context().as_raw();
    harness.ime.command(ImeCommand::Disable).unwrap();
    let focused_top = unsafe { harness.thread_mgr.GetFocus().and_then(|d| d.GetTop()) };
    assert!(focused_top.map(|c| c.as_raw() != ours).unwrap_or(true));
    harness
        .ime
        .command(ImeCommand::Enable(document("y", (0, 0))))
        .unwrap();
    assert_eq!(harness.context().as_raw(), ours);
}
