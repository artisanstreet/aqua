use crate::event::ShellError;
use crate::event::{win32, FileDrag, ShellEvent};
use crate::state;
use crate::wndproc::{hwnd_from, WM_WAKE};
use aqua_core::Point;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows::core::{implement, Ref};
use windows::Win32::Foundation::{HWND, POINT, POINTL, S_OK};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium,
    RevokeDragDrop, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, DragQueryFileW, IDropTargetHelper, HDROP};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn carries_files(object: &IDataObject) -> bool {
    let status = unsafe { object.QueryGetData(&hdrop_format()) };
    status == S_OK
}

fn read_paths(object: &IDataObject) -> Vec<PathBuf> {
    let Ok(mut medium) = (unsafe { object.GetData(&hdrop_format()) }) else {
        return Vec::new();
    };
    let hdrop = HDROP(unsafe { medium.u.hGlobal }.0);
    let paths = match hdrop.0.is_null() {
        true => Vec::new(),
        false => {
            let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
            (0..count)
                .map(|index| {
                    let length = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
                    let mut buffer = vec![0u16; length + 1];
                    let written =
                        unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) } as usize;
                    PathBuf::from(std::ffi::OsString::from_wide(&buffer[..written]))
                })
                .collect()
        }
    };
    unsafe { ReleaseStgMedium(&mut medium) };
    paths
}

#[implement(IDropTarget)]
struct Target {
    hwnd: isize,
    helper: Option<IDropTargetHelper>,
}

impl Target {
    fn position(&self, point: &POINTL) -> Point {
        let mut client = POINT {
            x: point.x,
            y: point.y,
        };
        let _ = unsafe { ScreenToClient(hwnd_from(self.hwnd), &mut client) };
        let scale = state::window(self.hwnd, |w| w.scale).unwrap_or(1.0);
        Point::new(
            (f64::from(client.x) / scale) as f32,
            (f64::from(client.y) / scale) as f32,
        )
    }

    fn emit(&self, drag: FileDrag) {
        state::emit(self.hwnd, ShellEvent::FileDrag(drag));
        let _ = unsafe {
            PostMessageW(
                Some(hwnd_from(self.hwnd)),
                WM_WAKE,
                Default::default(),
                Default::default(),
            )
        };
    }

    fn helper_point(point: &POINTL) -> POINT {
        POINT {
            x: point.x,
            y: point.y,
        }
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        object: Ref<IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let object = object.ok()?;
        let files = carries_files(object);
        let result = if files {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        };
        unsafe { *effect = result };
        if files {
            self.emit(FileDrag::Enter {
                paths: read_paths(object),
                position: self.position(point),
            });
        }
        if let Some(helper) = &self.helper {
            let _ = unsafe {
                helper.DragEnter(
                    hwnd_from(self.hwnd),
                    object,
                    &Target::helper_point(point),
                    result,
                )
            };
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe { *effect = DROPEFFECT_COPY };
        if let Some(helper) = &self.helper {
            let _ = unsafe { helper.DragOver(&Target::helper_point(point), DROPEFFECT_COPY) };
        }
        self.emit(FileDrag::Over {
            position: self.position(point),
        });
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if let Some(helper) = &self.helper {
            let _ = unsafe { helper.DragLeave() };
        }
        self.emit(FileDrag::Leave);
        Ok(())
    }

    fn Drop(
        &self,
        object: Ref<IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let object = object.ok()?;
        let files = carries_files(object);
        unsafe {
            *effect = if files {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            }
        };
        if let Some(helper) = &self.helper {
            let _ = unsafe { helper.Drop(object, &Target::helper_point(point), DROPEFFECT_COPY) };
        }
        match files {
            true => self.emit(FileDrag::Drop {
                paths: read_paths(object),
                position: self.position(point),
            }),
            false => self.emit(FileDrag::Leave),
        }
        Ok(())
    }
}

pub(crate) fn register(hwnd: HWND) -> Result<(), ShellError> {
    unsafe { OleInitialize(None) }.map_err(|e| win32("OleInitialize", e))?;
    let helper: Option<IDropTargetHelper> =
        unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
    let target: IDropTarget = Target {
        hwnd: hwnd.0 as isize,
        helper,
    }
    .into();
    unsafe { RegisterDragDrop(hwnd, &target) }.map_err(|e| win32("RegisterDragDrop", e))
}

pub(crate) fn revoke(hwnd: HWND) {
    let _ = unsafe { RevokeDragDrop(hwnd) };
}
