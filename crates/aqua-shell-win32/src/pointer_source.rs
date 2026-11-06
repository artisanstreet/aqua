use crate::pointer::Device;
use std::cell::RefCell;
use windows::Win32::UI::Input::Pointer::{
    GetPointerInfo, GetPointerPenInfo, GetPointerType, POINTER_INFO, POINTER_PEN_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::POINTER_INPUT_TYPE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub device: Device,
    pub flags: u32,
    pub pen_flags: u32,
    pub screen: (i32, i32),
}

pub trait PointerSource: 'static {
    fn read(&mut self, id: u32) -> Option<Reading>;
}

thread_local! {
    static SOURCE: RefCell<Option<Box<dyn PointerSource>>> = const { RefCell::new(None) };
}

pub fn set_pointer_source(source: impl PointerSource) {
    SOURCE.with(|slot| *slot.borrow_mut() = Some(Box::new(source)));
}

pub fn clear_pointer_source() {
    SOURCE.with(|slot| *slot.borrow_mut() = None);
}

pub(crate) fn kind_of(id: u32) -> Option<Device> {
    let injected = SOURCE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(|source| source.read(id).map(|reading| reading.device))
    });
    match injected {
        Some(kind) => kind,
        None => {
            let mut kind = POINTER_INPUT_TYPE::default();
            unsafe { GetPointerType(id, &mut kind) }.ok()?;
            Device::from_pointer_type(kind.0)
        }
    }
}

pub(crate) fn read(id: u32, device: Device) -> Option<Reading> {
    let injected = SOURCE.with(|slot| slot.borrow_mut().as_mut().map(|source| source.read(id)));
    match injected {
        Some(reading) => reading,
        None => {
            let mut info = POINTER_INFO::default();
            unsafe { GetPointerInfo(id, &mut info) }.ok()?;
            let pen_flags = match device {
                Device::Pen => {
                    let mut pen = POINTER_PEN_INFO::default();
                    unsafe { GetPointerPenInfo(id, &mut pen) }
                        .map(|_| pen.penFlags)
                        .unwrap_or(0)
                }
                Device::Touch => 0,
            };
            Some(Reading {
                device,
                flags: info.pointerFlags.0,
                pen_flags,
                screen: (info.ptPixelLocation.x, info.ptPixelLocation.y),
            })
        }
    }
}
