use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::budget::MemoryBudget;
use super::error::Error;

pub struct BudgetMonitor {
    adapter: IDXGIAdapter3,
    event: HANDLE,
    cookie: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub local: MemoryBudget,
    pub non_local: MemoryBudget,
}

fn query(adapter: &IDXGIAdapter3, group: DXGI_MEMORY_SEGMENT_GROUP) -> Result<MemoryBudget, Error> {
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    unsafe { adapter.QueryVideoMemoryInfo(0, group, &mut info)? };
    Ok(MemoryBudget {
        budget: info.Budget,
        usage: info.CurrentUsage,
    })
}

impl BudgetMonitor {
    pub fn new(adapter: &IDXGIAdapter3) -> Result<Self, Error> {
        let event = unsafe { CreateEventW(None, false, false, None)? };
        let cookie = unsafe { adapter.RegisterVideoMemoryBudgetChangeNotificationEvent(event)? };
        Ok(Self {
            adapter: adapter.clone(),
            event,
            cookie,
        })
    }

    pub fn changed(&self) -> bool {
        unsafe { WaitForSingleObject(self.event, 0) == WAIT_OBJECT_0 }
    }

    pub fn snapshot(&self) -> Result<MemorySnapshot, Error> {
        Ok(MemorySnapshot {
            local: query(&self.adapter, DXGI_MEMORY_SEGMENT_GROUP_LOCAL)?,
            non_local: query(&self.adapter, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL)?,
        })
    }

    pub fn reserve(&self, local_bytes: u64) -> Result<(), Error> {
        unsafe {
            self.adapter.SetVideoMemoryReservation(
                0,
                DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
                local_bytes,
            )?
        };
        Ok(())
    }
}

impl Drop for BudgetMonitor {
    fn drop(&mut self) {
        unsafe {
            self.adapter
                .UnregisterVideoMemoryBudgetChangeNotification(self.cookie);
            let _ = CloseHandle(self.event);
        }
    }
}

pub fn evict(device: &ID3D12Device, objects: &[ID3D12Pageable]) -> Result<(), Error> {
    let list: Vec<Option<ID3D12Pageable>> = objects.iter().cloned().map(Some).collect();
    unsafe { device.Evict(&list)? };
    Ok(())
}

pub fn make_resident(device: &ID3D12Device, objects: &[ID3D12Pageable]) -> Result<(), Error> {
    let list: Vec<Option<ID3D12Pageable>> = objects.iter().cloned().map(Some).collect();
    unsafe { device.MakeResident(&list)? };
    Ok(())
}
