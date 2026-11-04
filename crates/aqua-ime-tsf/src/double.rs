use crate::TsfIme;
use aqua_protocol::{ImeBackend, ImeCommand, ImeEvent};
use std::cell::RefCell;
use std::rc::Rc;
use windows::core::{implement, Interface, Ref, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
    ITfContextComposition, ITfEditSession, ITfEditSession_Impl, ITfThreadMgr, TF_DEFAULT_SELECTION,
    TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
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

#[implement(ITfCompositionSink)]
struct Sink;

impl ITfCompositionSink_Impl for Sink_Impl {
    fn OnCompositionTerminated(&self, _: u32, _: Ref<ITfComposition>) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoubleError {
    Window,
    NoFocusedDocument,
    Tsf(i32),
}

impl From<windows::core::Error> for DoubleError {
    fn from(error: windows::core::Error) -> Self {
        Self::Tsf(error.code().0)
    }
}

pub struct TsfDouble {
    hwnd: HWND,
    ime: TsfIme,
    thread_mgr: ITfThreadMgr,
    client: u32,
}

impl TsfDouble {
    pub fn new() -> std::result::Result<Self, DoubleError> {
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                windows::core::w!("STATIC"),
                windows::core::w!("aqua tsf double"),
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
        .map_err(|_| DoubleError::Window)?;
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        let ime = TsfIme::from_hwnd(hwnd.0 as isize);
        let thread_mgr: ITfThreadMgr =
            unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }?;
        let client = unsafe { thread_mgr.Activate() }?;
        Ok(Self {
            hwnd,
            ime,
            thread_mgr,
            client,
        })
    }

    pub fn command(&mut self, command: ImeCommand) -> std::result::Result<(), crate::TsfError> {
        self.ime.command(command)
    }

    pub fn drain(&mut self, out: &mut Vec<ImeEvent>) {
        self.ime.drain(out)
    }

    fn edit<T: 'static>(
        &self,
        body: impl FnOnce(&ITfContext, u32) -> Result<T> + 'static,
    ) -> std::result::Result<T, DoubleError> {
        let document =
            unsafe { self.thread_mgr.GetFocus() }.map_err(|_| DoubleError::NoFocusedDocument)?;
        let context = unsafe { document.GetTop() }?;
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
        }?;
        hr.ok()?;
        let result = slot.borrow_mut().take();
        result.ok_or(DoubleError::Tsf(-1))
    }

    pub fn compose(&self, steps: &[&str], end: bool) -> std::result::Result<(), DoubleError> {
        let steps: Vec<Vec<u16>> = steps
            .iter()
            .map(|step| step.encode_utf16().collect())
            .collect();
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
            let sink: ITfCompositionSink = Sink.into();
            let composition = unsafe { api.StartComposition(cookie, &range, &sink) }?;
            let inner = unsafe { composition.GetRange() }?;
            for step in &steps {
                unsafe { inner.SetText(cookie, 0, step) }?;
            }
            if end {
                unsafe { composition.EndComposition(cookie) }?;
            }
            Ok(())
        })
    }
}

impl Drop for TsfDouble {
    fn drop(&mut self) {
        let _ = self.ime.command(ImeCommand::Disable);
        let _ = unsafe { self.thread_mgr.Deactivate() };
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}
