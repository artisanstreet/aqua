use crate::model::Model;
use crate::session::Session;
use crate::store::{Shared, Store};
use aqua_protocol::{ImeBackend, ImeCommand, ImeDocument, ImeEvent};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::Cell;
use std::rc::Rc;
use windows::Win32::Foundation::{HWND, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, GUID_SYSTEM_FUNCTIONPROVIDER, ITextStoreACP, ITfContext,
    ITfContextOwnerCompositionServices, ITfDocumentMgr, ITfFnReconversion, ITfThreadMgr,
    TF_DEFAULT_SELECTION, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
};
use windows::core::{BOOL, Error, Interface};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsfError {
    NoWindowHandle,
    ComApartment,
    Win32(&'static str, i32),
    NotReconvertible,
}

impl std::fmt::Display for TsfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TsfError::NoWindowHandle => write!(f, "window has no Win32 handle"),
            TsfError::ComApartment => write!(f, "thread is not a single-threaded COM apartment"),
            TsfError::Win32(call, code) => write!(f, "{call} failed with {code:#x}"),
            TsfError::NotReconvertible => write!(f, "selection cannot be reconverted"),
        }
    }
}

impl std::error::Error for TsfError {}

fn win32(call: &'static str) -> impl Fn(Error) -> TsfError {
    move |error| TsfError::Win32(call, error.code().0)
}

struct Active {
    thread_mgr: ITfThreadMgr,
    client_id: u32,
    document_mgr: ITfDocumentMgr,
    idle_mgr: ITfDocumentMgr,
    context: ITfContext,
    shared: Rc<Shared>,
    enabled: bool,
    _store: ITextStoreACP,
}

pub struct TsfIme {
    hwnd: HWND,
    active: Option<Active>,
}

impl TsfIme {
    pub fn new(window: &impl HasWindowHandle) -> Result<Self, TsfError> {
        let handle = window
            .window_handle()
            .map_err(|_| TsfError::NoWindowHandle)?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Ok(Self::from_hwnd(handle.hwnd.get())),
            _ => Err(TsfError::NoWindowHandle),
        }
    }

    pub fn from_hwnd(hwnd: isize) -> Self {
        Self {
            hwnd: HWND(hwnd as *mut _),
            active: None,
        }
    }

    fn activate(&mut self, document: &ImeDocument) -> Result<&mut Active, TsfError> {
        if self.active.is_none() {
            self.active = Some(build(self.hwnd, document)?);
        }
        Ok(self.active.as_mut().expect("activated above"))
    }
}

fn build(hwnd: HWND, document: &ImeDocument) -> Result<Active, TsfError> {
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init == RPC_E_CHANGED_MODE {
        return Err(TsfError::ComApartment);
    }
    let thread_mgr: ITfThreadMgr =
        unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }
            .map_err(win32("CoCreateInstance(ThreadMgr)"))?;
    let client_id = unsafe { thread_mgr.Activate() }.map_err(win32("ITfThreadMgr::Activate"))?;
    let document_mgr =
        unsafe { thread_mgr.CreateDocumentMgr() }.map_err(win32("CreateDocumentMgr"))?;
    let idle_mgr = unsafe { thread_mgr.CreateDocumentMgr() }.map_err(win32("CreateDocumentMgr"))?;
    let shared = Shared::new(Model::new(document), hwnd);
    let store: ITextStoreACP = Store {
        shared: shared.clone(),
    }
    .into();
    let unknown: windows::core::IUnknown = store.cast().map_err(win32("store->IUnknown"))?;
    let mut context = None;
    let mut edit_cookie = 0u32;
    unsafe { document_mgr.CreateContext(client_id, 0, &unknown, &mut context, &mut edit_cookie) }
        .map_err(win32("CreateContext"))?;
    let context = context.ok_or(TsfError::Win32("CreateContext", -1))?;
    unsafe { document_mgr.Push(&context) }.map_err(win32("Push"))?;
    Ok(Active {
        thread_mgr,
        client_id,
        document_mgr,
        idle_mgr,
        context,
        shared,
        enabled: false,
        _store: store,
    })
}

fn associate(
    thread_mgr: &ITfThreadMgr,
    hwnd: HWND,
    document: Option<&ITfDocumentMgr>,
) -> Result<(), TsfError> {
    match unsafe { thread_mgr.AssociateFocus(hwnd, document) } {
        Ok(_) => Ok(()),
        Err(error) if error.code().is_ok() => Ok(()),
        Err(error) => Err(win32("AssociateFocus")(error)),
    }
}

impl Active {
    fn focus(&mut self, hwnd: HWND) -> Result<(), TsfError> {
        unsafe {
            associate(&self.thread_mgr, hwnd, Some(&self.document_mgr))?;
            self.thread_mgr
                .SetFocus(&self.document_mgr)
                .map_err(win32("SetFocus"))?;
        }
        self.enabled = true;
        Ok(())
    }

    fn unfocus(&mut self, hwnd: HWND) -> Result<(), TsfError> {
        self.terminate();
        unsafe {
            associate(&self.thread_mgr, hwnd, Some(&self.idle_mgr))?;
            self.thread_mgr
                .SetFocus(&self.idle_mgr)
                .map_err(win32("SetFocus(idle)"))?;
        }
        self.enabled = false;
        Ok(())
    }

    fn terminate(&self) {
        if let Ok(services) = self.context.cast::<ITfContextOwnerCompositionServices>() {
            let _ = unsafe { services.TerminateComposition(None) };
        }
    }

    fn cancel(&self) {
        let removed = self.shared.model.borrow_mut().discard_composition();
        if let Some(change) = removed {
            self.shared.notify(crate::model::Notify {
                text: Some(change),
                selection: true,
                layout: false,
            });
        }
        self.terminate();
    }

    fn reconvert(&self) -> Result<(), TsfError> {
        let provider = unsafe {
            self.thread_mgr
                .GetFunctionProvider(&GUID_SYSTEM_FUNCTIONPROVIDER)
        }
        .map_err(win32("GetFunctionProvider"))?;
        let function: ITfFnReconversion = unsafe {
            provider.GetFunction(&windows::core::GUID::zeroed(), &ITfFnReconversion::IID)
        }
        .and_then(|unknown| unknown.cast())
        .map_err(win32("GetFunction(Reconversion)"))?;
        let outcome = Rc::new(Cell::new(Err(TsfError::NotReconvertible)));
        let context = self.context.clone();
        let slot = outcome.clone();
        let session = Session::new(move |cookie| {
            let mut selection = [TF_SELECTION::default()];
            let mut fetched = 0u32;
            unsafe {
                context.GetSelection(cookie, TF_DEFAULT_SELECTION, &mut selection, &mut fetched)
            }?;
            let range = selection[0]
                .range
                .take()
                .ok_or_else(|| Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
            let mut target = None;
            let mut convertible = BOOL(0);
            unsafe { function.QueryRange(&range, &mut target, &mut convertible) }?;
            if let (true, Some(target)) = (convertible.as_bool(), target) {
                unsafe { function.Reconvert(&target) }?;
                slot.set(Ok(()));
            }
            Ok(())
        });
        let hr = unsafe {
            self.context
                .RequestEditSession(self.client_id, &session, TF_ES_READWRITE | TF_ES_SYNC)
        }
        .map_err(win32("RequestEditSession"))?;
        if hr.is_err() {
            return Err(TsfError::Win32("RequestEditSession", hr.0));
        }
        outcome.replace(Err(TsfError::NotReconvertible))
    }
}

impl ImeBackend for TsfIme {
    type Error = TsfError;

    fn command(&mut self, command: ImeCommand) -> Result<(), TsfError> {
        let hwnd = self.hwnd;
        match command {
            ImeCommand::Enable(document) => {
                let active = self.activate(&document)?;
                if active.enabled {
                    active.cancel();
                }
                let notify = active.shared.model.borrow_mut().apply_document(&document);
                active.shared.notify(notify);
                if !active.enabled {
                    active.focus(hwnd)?;
                }
                Ok(())
            }
            ImeCommand::Update(document) => match self.active.as_mut().filter(|a| a.enabled) {
                Some(active) => {
                    let notify = active.shared.model.borrow_mut().apply_document(&document);
                    active.shared.notify(notify);
                    Ok(())
                }
                None => Ok(()),
            },
            ImeCommand::Disable => match self.active.as_mut().filter(|a| a.enabled) {
                Some(active) => active.unfocus(hwnd),
                None => Ok(()),
            },
            ImeCommand::Reconvert => match self.active.as_ref().filter(|a| a.enabled) {
                Some(active) => active.reconvert(),
                None => Err(TsfError::NotReconvertible),
            },
            ImeCommand::CommitComposition => {
                if let Some(active) = self.active.as_ref() {
                    active.terminate();
                }
                Ok(())
            }
            ImeCommand::CancelComposition => {
                if let Some(active) = self.active.as_ref() {
                    active.cancel();
                }
                Ok(())
            }
        }
    }

    fn drain(&mut self, out: &mut Vec<ImeEvent>) {
        if let Some(active) = self.active.as_ref() {
            active.shared.model.borrow_mut().drain(out);
        }
    }
}

impl Drop for TsfIme {
    fn drop(&mut self) {
        if let Some(active) = self.active.take() {
            let hwnd = self.hwnd;
            let mut active = active;
            if active.enabled {
                let _ = active.unfocus(hwnd);
            }
            let _ = unsafe {
                active
                    .document_mgr
                    .Pop(windows::Win32::UI::TextServices::TF_POPF_ALL)
            };
            let _ = unsafe { active.thread_mgr.Deactivate() };
        }
    }
}
