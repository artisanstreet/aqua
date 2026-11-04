use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::error::Error;

pub const WAIT_LIMIT_MS: u32 = 10_000;

pub struct Timeline {
    fence: ID3D12Fence,
    event: HANDLE,
    value: u64,
}

impl Timeline {
    pub fn new(device: &ID3D12Device) -> Result<Self, Error> {
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        let event = unsafe { CreateEventW(None, false, false, None)? };
        Ok(Self {
            fence,
            event,
            value: 0,
        })
    }

    pub fn next_value(&self) -> u64 {
        self.value + 1
    }

    pub fn completed(&self) -> u64 {
        unsafe { self.fence.GetCompletedValue() }
    }

    pub fn is_poisoned(&self) -> bool {
        self.completed() == u64::MAX
    }

    pub fn signal(&mut self, queue: &ID3D12CommandQueue) -> Result<u64, Error> {
        self.value += 1;
        unsafe { queue.Signal(&self.fence, self.value)? };
        Ok(self.value)
    }

    pub fn wait(&self, value: u64) -> Result<(), Error> {
        match self.completed() >= value {
            true => Ok(()),
            false => {
                unsafe { self.fence.SetEventOnCompletion(value, self.event)? };
                match unsafe { WaitForSingleObject(self.event, WAIT_LIMIT_MS) } {
                    status if status == WAIT_OBJECT_0 => Ok(()),
                    _ => Err(Error::Timeout),
                }
            }
        }
    }

    pub fn wait_idle(&mut self, queue: &ID3D12CommandQueue) -> Result<(), Error> {
        let value = self.signal(queue)?;
        self.wait(value)
    }
}

impl Drop for Timeline {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}
