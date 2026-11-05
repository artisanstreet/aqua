use crate::event::{win32, ShellError};
use windows::core::IUnknown;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;

pub(crate) struct Composition {
    device: IDCompositionDevice,
    visual: IDCompositionVisual,
    _target: IDCompositionTarget,
}

impl Composition {
    pub(crate) fn create(hwnd: HWND) -> Result<Composition, ShellError> {
        let device: IDCompositionDevice = unsafe { DCompositionCreateDevice(None::<&IDXGIDevice>) }
            .map_err(|e| win32("DCompositionCreateDevice", e))?;
        let target = unsafe { device.CreateTargetForHwnd(hwnd, true) }
            .map_err(|e| win32("CreateTargetForHwnd", e))?;
        let visual = unsafe { device.CreateVisual() }.map_err(|e| win32("CreateVisual", e))?;
        unsafe { target.SetRoot(&visual) }.map_err(|e| win32("IDCompositionTarget::SetRoot", e))?;
        Ok(Composition {
            device,
            visual,
            _target: target,
        })
    }

    pub(crate) fn set_content(&self, content: &IUnknown) -> Result<(), ShellError> {
        unsafe { self.visual.SetContent(content) }
            .map_err(|e| win32("IDCompositionVisual::SetContent", e))?;
        unsafe { self.device.Commit() }.map_err(|e| win32("IDCompositionDevice::Commit", e))
    }
}
