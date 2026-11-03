use std::marker::PhantomData;
use std::rc::Rc;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_SINGLETHREADED};

pub(crate) struct Apartment {
    owned: bool,
    thread: PhantomData<Rc<()>>,
}

impl Apartment {
    pub(crate) fn new() -> windows::core::Result<Self> {
        let owned = match unsafe { RoInitialize(RO_INIT_SINGLETHREADED) } {
            Ok(()) => true,
            Err(error) if error.code() == RPC_E_CHANGED_MODE => false,
            Err(error) => return Err(error),
        };
        Ok(Self {
            owned,
            thread: PhantomData,
        })
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.owned {
            unsafe {
                RoUninitialize();
            }
        }
    }
}
