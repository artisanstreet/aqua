use crate::model::{EditError, Model, Notify, Selection, TextChange};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use windows::core::{implement, Error, Interface, Ref, Result, BOOL, GUID, HRESULT, PWSTR};
use windows::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, E_NOTIMPL, E_UNEXPECTED, HWND, POINT, RECT, S_OK,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{IDataObject, FORMATETC};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::TextServices::{
    ITextStoreACP, ITextStoreACPSink, ITextStoreACP_Impl, ITfCompositionView,
    ITfContextOwnerCompositionSink, ITfContextOwnerCompositionSink_Impl, ITfRange, ITfRangeACP,
    TEXT_STORE_LOCK_FLAGS, TEXT_STORE_TEXT_CHANGE_FLAGS, TS_AE_END, TS_AE_START,
    TS_AS_LAYOUT_CHANGE, TS_AS_SEL_CHANGE, TS_AS_TEXT_CHANGE, TS_ATTRVAL, TS_E_INVALIDPOS,
    TS_E_NOLOCK, TS_E_NOSELECTION, TS_E_READONLY, TS_E_SYNCHRONOUS, TS_IAS_QUERYONLY, TS_LC_CHANGE,
    TS_LF_SYNC, TS_RT_PLAIN, TS_RUNINFO, TS_SELECTIONSTYLE, TS_SELECTION_ACP, TS_SS_NOHIDDENTEXT,
    TS_STATUS, TS_S_ASYNC, TS_TEXTCHANGE,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

const CONNECT_E_NOCONNECTION: HRESULT = HRESULT(0x80040200_u32 as i32);
const CONNECT_E_ADVISELIMIT: HRESULT = HRESULT(0x80040201_u32 as i32);
const WRITE_BIT: u32 = 4;
const ACP_END: i32 = -1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lock {
    None,
    Read,
    Write,
}

pub(crate) struct Shared {
    pub model: RefCell<Model>,
    sink: RefCell<Option<ITextStoreACPSink>>,
    mask: Cell<u32>,
    lock: Cell<Lock>,
    pending_lock: Cell<u32>,
    deferred: RefCell<Notify>,
    hwnd: HWND,
}

impl Shared {
    pub(crate) fn new(model: Model, hwnd: HWND) -> Rc<Self> {
        Rc::new(Self {
            model: RefCell::new(model),
            sink: RefCell::new(None),
            mask: Cell::new(0),
            lock: Cell::new(Lock::None),
            pending_lock: Cell::new(0),
            deferred: RefCell::new(Notify::default()),
            hwnd,
        })
    }

    pub(crate) fn notify(&self, notify: Notify) {
        if notify.is_empty() {
            return;
        }
        if self.lock.get() != Lock::None {
            self.defer(notify);
            return;
        }
        self.deliver(notify);
    }

    fn defer(&self, notify: Notify) {
        let mut deferred = self.deferred.borrow_mut();
        deferred.text = match (deferred.text, notify.text) {
            (Some(a), Some(b)) => Some(TextChange {
                start: a.start.min(b.start),
                old_end: a.old_end.max(b.old_end),
                new_end: a.new_end.max(b.new_end),
            }),
            (a, b) => a.or(b),
        };
        deferred.selection |= notify.selection;
        deferred.layout |= notify.layout;
    }

    fn deliver(&self, notify: Notify) {
        let Some(sink) = self.sink.borrow().clone() else {
            return;
        };
        let mask = self.mask.get();
        if let (Some(change), true) = (notify.text, mask & TS_AS_TEXT_CHANGE != 0) {
            let change = ts_change(change);
            let _ = unsafe { sink.OnTextChange(TEXT_STORE_TEXT_CHANGE_FLAGS(0), &change) };
        }
        if notify.selection && mask & TS_AS_SEL_CHANGE != 0 {
            let _ = unsafe { sink.OnSelectionChange() };
        }
        if notify.layout && mask & TS_AS_LAYOUT_CHANGE != 0 {
            let _ = unsafe { sink.OnLayoutChange(TS_LC_CHANGE, 0) };
        }
    }

    fn grant(&self, flags: u32) -> HRESULT {
        let Some(sink) = self.sink.borrow().clone() else {
            return E_UNEXPECTED;
        };
        let mut flags = flags;
        let mut result;
        loop {
            self.lock.set(match flags & WRITE_BIT != 0 {
                true => Lock::Write,
                false => Lock::Read,
            });
            result = unsafe { sink.OnLockGranted(TEXT_STORE_LOCK_FLAGS(flags)) }
                .map(|_| S_OK)
                .unwrap_or_else(|e| e.code());
            self.lock.set(Lock::None);
            let deferred = std::mem::take(&mut *self.deferred.borrow_mut());
            self.deliver(deferred);
            match self.pending_lock.replace(0) {
                0 => break,
                pending => flags = pending,
            }
        }
        result
    }

    fn require(&self, needed: Lock) -> Result<()> {
        match (self.lock.get(), needed) {
            (Lock::None, _) | (Lock::Read, Lock::Write) => Err(Error::from_hresult(TS_E_NOLOCK)),
            _ => Ok(()),
        }
    }

    fn screen_rect(&self) -> RECT {
        let area = self.model.borrow().area();
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96) as f32 / 96.0;
        let mut origin = POINT { x: 0, y: 0 };
        let _ = unsafe { ClientToScreen(self.hwnd, &mut origin) };
        let left = origin.x + (area.x * dpi).round() as i32;
        let top = origin.y + (area.y * dpi).round() as i32;
        RECT {
            left,
            top,
            right: left + ((area.w * dpi).round() as i32).max(1),
            bottom: top + ((area.h * dpi).round() as i32).max(1),
        }
    }
}

fn ts_change(change: TextChange) -> TS_TEXTCHANGE {
    TS_TEXTCHANGE {
        acpStart: change.start as i32,
        acpOldEnd: change.old_end as i32,
        acpNewEnd: change.new_end as i32,
    }
}

fn map(error: EditError) -> Error {
    Error::from_hresult(match error {
        EditError::InvalidPosition => TS_E_INVALIDPOS,
        EditError::ReadOnlyWindow => TS_E_READONLY,
    })
}

fn position(value: i32) -> Result<u32> {
    u32::try_from(value).map_err(|_| Error::from_hresult(TS_E_INVALIDPOS))
}

fn extent(range: &ITfRange) -> Option<Range<u32>> {
    let acp = range.cast::<ITfRangeACP>().ok()?;
    let (mut anchor, mut count) = (0i32, 0i32);
    unsafe { acp.GetExtent(&mut anchor, &mut count) }.ok()?;
    let start = u32::try_from(anchor).ok()?;
    let len = u32::try_from(count).ok()?;
    Some(start..start + len)
}

#[implement(ITextStoreACP, ITfContextOwnerCompositionSink)]
pub(crate) struct Store {
    pub shared: Rc<Shared>,
}

impl ITextStoreACP_Impl for Store_Impl {
    fn AdviseSink(
        &self,
        riid: *const GUID,
        punk: Ref<windows::core::IUnknown>,
        mask: u32,
    ) -> Result<()> {
        if unsafe { *riid } != ITextStoreACPSink::IID {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let sink: ITextStoreACPSink = punk.ok()?.cast()?;
        let shared = &self.shared;
        let mut slot = shared.sink.borrow_mut();
        match slot.as_ref() {
            Some(existing) if existing.as_raw() != sink.as_raw() => {
                return Err(Error::from_hresult(CONNECT_E_ADVISELIMIT));
            }
            _ => {}
        }
        *slot = Some(sink);
        shared.mask.set(mask);
        Ok(())
    }

    fn UnadviseSink(&self, punk: Ref<windows::core::IUnknown>) -> Result<()> {
        let sink: ITextStoreACPSink = punk.ok()?.cast()?;
        let mut slot = self.shared.sink.borrow_mut();
        match slot.as_ref() {
            Some(existing) if existing.as_raw() == sink.as_raw() => {
                *slot = None;
                self.shared.mask.set(0);
                Ok(())
            }
            _ => Err(Error::from_hresult(CONNECT_E_NOCONNECTION)),
        }
    }

    fn RequestLock(&self, flags: u32) -> Result<HRESULT> {
        let shared = &self.shared;
        if shared.sink.borrow().is_none() {
            return Err(Error::from_hresult(E_UNEXPECTED));
        }
        if shared.lock.get() != Lock::None {
            if flags & TS_LF_SYNC != 0 {
                return Ok(TS_E_SYNCHRONOUS);
            }
            shared.pending_lock.set(shared.pending_lock.get() | flags);
            return Err(Error::from_hresult(TS_S_ASYNC));
        }
        Ok(shared.grant(flags))
    }

    fn GetStatus(&self) -> Result<TS_STATUS> {
        Ok(TS_STATUS {
            dwDynamicFlags: 0,
            dwStaticFlags: TS_SS_NOHIDDENTEXT,
        })
    }

    fn QueryInsert(
        &self,
        start: i32,
        end: i32,
        _cch: u32,
        result_start: *mut i32,
        result_end: *mut i32,
    ) -> Result<()> {
        let total = self.shared.model.borrow().total_len() as i32;
        if start < 0 || end < start {
            return Err(Error::from_hresult(TS_E_INVALIDPOS));
        }
        unsafe {
            *result_start = start.min(total);
            *result_end = end.min(total);
        }
        Ok(())
    }

    fn GetSelection(
        &self,
        index: u32,
        count: u32,
        selection: *mut TS_SELECTION_ACP,
        fetched: *mut u32,
    ) -> Result<()> {
        self.shared.require(Lock::Read)?;
        unsafe { *fetched = 0 };
        if count == 0 {
            return Ok(());
        }
        if index != u32::MAX && index != 0 {
            return Err(Error::from_hresult(TS_E_NOSELECTION));
        }
        let current = self.shared.model.borrow().selection();
        unsafe {
            *selection = TS_SELECTION_ACP {
                acpStart: current.start as i32,
                acpEnd: current.end as i32,
                style: TS_SELECTIONSTYLE {
                    ase: if current.end_active {
                        TS_AE_END
                    } else {
                        TS_AE_START
                    },
                    fInterimChar: BOOL(0),
                },
            };
            *fetched = 1;
        }
        Ok(())
    }

    fn SetSelection(&self, count: u32, selection: *const TS_SELECTION_ACP) -> Result<()> {
        self.shared.require(Lock::Write)?;
        if count != 1 {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let selection = unsafe { *selection };
        let next = Selection {
            start: position(selection.acpStart)?,
            end: position(selection.acpEnd)?,
            end_active: selection.style.ase != TS_AE_START,
        };
        self.shared
            .model
            .borrow_mut()
            .set_selection(next)
            .map_err(map)
    }

    fn GetText(
        &self,
        start: i32,
        end: i32,
        text: PWSTR,
        text_capacity: u32,
        text_len: *mut u32,
        runs: *mut TS_RUNINFO,
        run_capacity: u32,
        run_len: *mut u32,
        next: *mut i32,
    ) -> Result<()> {
        self.shared.require(Lock::Read)?;
        let model = self.shared.model.borrow();
        let start = position(start)?;
        let end = match end {
            ACP_END => None,
            other => Some(position(other)?),
        };
        let (units, text_start) = model.get_text(start, end).map_err(map)?;
        let mut copied = units.len().min(text_capacity as usize);
        if copied < units.len() && copied > 0 && (0xD800..0xDC00).contains(&units[copied - 1]) {
            copied -= 1;
        }
        let consumed_to = text_start + copied as u32;
        unsafe {
            if !text.is_null() && copied > 0 {
                std::ptr::copy_nonoverlapping(units.as_ptr(), text.0, copied);
            }
            *text_len = copied as u32;
            let run_count = u32::from(copied > 0 && run_capacity > 0 && !runs.is_null());
            if run_count == 1 {
                *runs = TS_RUNINFO {
                    uCount: copied as u32,
                    r#type: TS_RT_PLAIN,
                };
            }
            *run_len = run_count;
            *next = consumed_to as i32;
        }
        Ok(())
    }

    fn SetText(
        &self,
        _flags: u32,
        start: i32,
        end: i32,
        text: &windows::core::PCWSTR,
        cch: u32,
    ) -> Result<TS_TEXTCHANGE> {
        self.shared.require(Lock::Write)?;
        let units = slice_of(text, cch);
        let change = self
            .shared
            .model
            .borrow_mut()
            .replace(position(start)?, position(end)?, units)
            .map_err(map)?;
        Ok(ts_change(change))
    }

    fn GetFormattedText(&self, _start: i32, _end: i32) -> Result<IDataObject> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn GetEmbedded(
        &self,
        _pos: i32,
        _service: *const GUID,
        _riid: *const GUID,
    ) -> Result<windows::core::IUnknown> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn QueryInsertEmbedded(
        &self,
        _service: *const GUID,
        _format: *const FORMATETC,
    ) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn InsertEmbedded(
        &self,
        _flags: u32,
        _start: i32,
        _end: i32,
        _data: Ref<IDataObject>,
    ) -> Result<TS_TEXTCHANGE> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn InsertTextAtSelection(
        &self,
        flags: u32,
        text: &windows::core::PCWSTR,
        cch: u32,
        start: *mut i32,
        end: *mut i32,
        change: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        let query_only = flags & TS_IAS_QUERYONLY != 0;
        self.shared
            .require(if query_only { Lock::Read } else { Lock::Write })?;
        let units = slice_of(text, cch);
        if query_only {
            let current = self.shared.model.borrow().selection();
            unsafe {
                if !start.is_null() {
                    *start = current.start as i32;
                }
                if !end.is_null() {
                    *end = current.start as i32 + units.len() as i32;
                }
            }
            return Ok(());
        }
        let (inserted_start, inserted_end, applied) = self
            .shared
            .model
            .borrow_mut()
            .insert_at_selection(units)
            .map_err(map)?;
        unsafe {
            if !start.is_null() {
                *start = inserted_start as i32;
            }
            if !end.is_null() {
                *end = inserted_end as i32;
            }
            if !change.is_null() {
                *change = ts_change(applied);
            }
        }
        Ok(())
    }

    fn InsertEmbeddedAtSelection(
        &self,
        _flags: u32,
        _data: Ref<IDataObject>,
        _start: *mut i32,
        _end: *mut i32,
        _change: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn RequestSupportedAttrs(&self, _flags: u32, _count: u32, _attrs: *const GUID) -> Result<()> {
        Ok(())
    }

    fn RequestAttrsAtPosition(
        &self,
        _pos: i32,
        _count: u32,
        _attrs: *const GUID,
        _flags: u32,
    ) -> Result<()> {
        Ok(())
    }

    fn RequestAttrsTransitioningAtPosition(
        &self,
        _pos: i32,
        _count: u32,
        _attrs: *const GUID,
        _flags: u32,
    ) -> Result<()> {
        Ok(())
    }

    fn FindNextAttrTransition(
        &self,
        _start: i32,
        halt: i32,
        _count: u32,
        _attrs: *const GUID,
        _flags: u32,
        next: *mut i32,
        found: *mut BOOL,
        offset: *mut i32,
    ) -> Result<()> {
        unsafe {
            *next = halt;
            *found = BOOL(0);
            *offset = 0;
        }
        Ok(())
    }

    fn RetrieveRequestedAttrs(
        &self,
        _count: u32,
        _values: *mut TS_ATTRVAL,
        fetched: *mut u32,
    ) -> Result<()> {
        unsafe { *fetched = 0 };
        Ok(())
    }

    fn GetEndACP(&self) -> Result<i32> {
        self.shared.require(Lock::Read)?;
        Ok(self.shared.model.borrow().total_len() as i32)
    }

    fn GetActiveView(&self) -> Result<u32> {
        Ok(0)
    }

    fn GetACPFromPoint(&self, _view: u32, _point: *const POINT, _flags: u32) -> Result<i32> {
        let model = self.shared.model.borrow();
        Ok(model
            .composition()
            .map(|c| c.start)
            .unwrap_or(model.selection().start) as i32)
    }

    fn GetTextExt(
        &self,
        _view: u32,
        _start: i32,
        _end: i32,
        rect: *mut RECT,
        clipped: *mut BOOL,
    ) -> Result<()> {
        self.shared.require(Lock::Read)?;
        unsafe {
            *rect = self.shared.screen_rect();
            *clipped = BOOL(0);
        }
        Ok(())
    }

    fn GetScreenExt(&self, _view: u32) -> Result<RECT> {
        let hwnd = self.shared.hwnd;
        let mut client = RECT::default();
        unsafe { GetClientRect(hwnd, &mut client) }.map_err(|_| Error::from_hresult(E_FAIL))?;
        let mut origin = POINT { x: 0, y: 0 };
        let _ = unsafe { ClientToScreen(hwnd, &mut origin) };
        Ok(RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + client.right,
            bottom: origin.y + client.bottom,
        })
    }

    fn GetWnd(&self, _view: u32) -> Result<HWND> {
        Ok(self.shared.hwnd)
    }
}

fn slice_of<'a>(text: &windows::core::PCWSTR, cch: u32) -> &'a [u16] {
    match text.is_null() || cch == 0 {
        true => &[],
        false => unsafe { std::slice::from_raw_parts(text.0, cch as usize) },
    }
}

impl ITfContextOwnerCompositionSink_Impl for Store_Impl {
    fn OnStartComposition(&self, composition: Ref<ITfCompositionView>) -> Result<BOOL> {
        let range = composition
            .as_ref()
            .and_then(|view| unsafe { view.GetRange() }.ok())
            .and_then(|range| extent(&range));
        self.shared.model.borrow_mut().start_composition(range);
        Ok(BOOL(1))
    }

    fn OnUpdateComposition(
        &self,
        composition: Ref<ITfCompositionView>,
        range: Ref<ITfRange>,
    ) -> Result<()> {
        let from_new = range.as_ref().and_then(extent);
        let range = from_new.or_else(|| {
            composition
                .as_ref()
                .and_then(|view| unsafe { view.GetRange() }.ok())
                .and_then(|range| extent(&range))
        });
        if let Some(range) = range {
            self.shared.model.borrow_mut().update_composition(range);
        }
        Ok(())
    }

    fn OnEndComposition(&self, _composition: Ref<ITfCompositionView>) -> Result<()> {
        self.shared.model.borrow_mut().end_composition();
        Ok(())
    }
}
