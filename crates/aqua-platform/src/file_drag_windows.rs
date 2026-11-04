use std::marker::PhantomData;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::rc::Rc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::{implement, Ref};
use windows::Win32::Foundation::{E_HANDLE, HWND, POINT, POINTL, S_OK};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, OleInitialize, OleUninitialize, RegisterDragDrop,
    ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, DragQueryFileW, IDropTargetHelper, HDROP};

use crate::events::ExternalFileDrag;

#[implement(IDropTarget)]
struct Target {
    hwnd: HWND,
    helper: IDropTargetHelper,
    send: Rc<dyn Fn(ExternalFileDrag)>,
}

impl Target {
    fn position(&self, point: POINTL) -> aqua_core::Point {
        let mut client = POINT {
            x: point.x,
            y: point.y,
        };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut client);
        }
        let scale = (unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0).max(0.01);
        aqua_core::Point::new(client.x as f32 / scale, client.y as f32 / scale)
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
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let valid = unsafe { object.QueryGetData(&format) } == S_OK;
        let result = if valid {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        };
        unsafe {
            *effect = result;
        }
        if valid {
            if let Ok(mut medium) = unsafe { object.GetData(&format) } {
                let hdrop = HDROP(unsafe { medium.u.hGlobal }.0);
                if !hdrop.0.is_null() {
                    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
                    let mut paths = Vec::with_capacity(count as usize);
                    for index in 0..count {
                        let length = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
                        let mut path = vec![0u16; length + 1];
                        let written =
                            unsafe { DragQueryFileW(hdrop, index, Some(&mut path)) } as usize;
                        paths.push(PathBuf::from(std::ffi::OsString::from_wide(
                            &path[..written],
                        )));
                    }
                    (self.send)(ExternalFileDrag::Enter {
                        paths,
                        position: self.position(*point),
                    });
                }
                unsafe {
                    ReleaseStgMedium(&mut medium);
                }
            }
        }
        let screen = POINT {
            x: point.x,
            y: point.y,
        };
        unsafe {
            let _ = self.helper.DragEnter(self.hwnd, object, &screen, result);
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            *effect = DROPEFFECT_COPY;
        }
        let screen = POINT {
            x: point.x,
            y: point.y,
        };
        unsafe {
            let _ = self.helper.DragOver(&screen, DROPEFFECT_COPY);
        }
        (self.send)(ExternalFileDrag::Move {
            position: self.position(*point),
        });
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        unsafe {
            let _ = self.helper.DragLeave();
        }
        (self.send)(ExternalFileDrag::Exit);
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
        unsafe {
            *effect = DROPEFFECT_COPY;
        }
        let screen = POINT {
            x: point.x,
            y: point.y,
        };
        unsafe {
            let _ = self.helper.Drop(object, &screen, DROPEFFECT_COPY);
        }
        (self.send)(ExternalFileDrag::Submit {
            position: self.position(*point),
        });
        Ok(())
    }
}

pub struct NativeFileDrag {
    hwnd: HWND,
    _target: IDropTarget,
    thread: PhantomData<*mut ()>,
}

impl NativeFileDrag {
    pub fn install(
        window: &winit::window::Window,
        send: impl Fn(ExternalFileDrag) + 'static,
    ) -> windows::core::Result<Self> {
        let RawWindowHandle::Win32(handle) = window
            .window_handle()
            .map_err(|_| windows::core::Error::from(E_HANDLE))?
            .as_raw()
        else {
            return Err(windows::core::Error::from(E_HANDLE));
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        unsafe {
            OleInitialize(None)?;
        }
        let result = (|| {
            let helper: IDropTargetHelper =
                unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER)? };
            let target: IDropTarget = Target {
                hwnd,
                helper,
                send: Rc::new(send),
            }
            .into();
            unsafe {
                RegisterDragDrop(hwnd, &target)?;
            }
            Ok(Self {
                hwnd,
                _target: target,
                thread: PhantomData,
            })
        })();
        if result.is_err() {
            unsafe {
                OleUninitialize();
            }
        }
        result
    }
}

impl Drop for NativeFileDrag {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
            OleUninitialize();
        }
    }
}
