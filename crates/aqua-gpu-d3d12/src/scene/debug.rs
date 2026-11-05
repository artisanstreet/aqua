use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

pub fn requested() -> bool {
    std::env::var("AQUA_D3D12_DEBUG").as_deref() == Ok("1")
}

fn gpu_validation_requested() -> bool {
    std::env::var("AQUA_D3D12_GBV").as_deref() == Ok("1")
}

pub fn enable() {
    if !requested() {
        return;
    }
    let mut debug: Option<ID3D12Debug> = None;
    if unsafe { D3D12GetDebugInterface(&mut debug) }.is_err() {
        return;
    }
    let Some(debug) = debug else {
        return;
    };
    unsafe { debug.EnableDebugLayer() };
    if gpu_validation_requested() {
        if let Ok(debug1) = debug.cast::<ID3D12Debug1>() {
            unsafe { debug1.SetEnableGPUBasedValidation(true) };
        }
    }
}

fn message_text(queue: &ID3D12InfoQueue, index: u64) -> Option<String> {
    let mut length = 0usize;
    unsafe { queue.GetMessage(index, None, &mut length) }.ok()?;
    let mut storage = vec![0u64; length.div_ceil(8)];
    let message = storage.as_mut_ptr() as *mut D3D12_MESSAGE;
    unsafe { queue.GetMessage(index, Some(message), &mut length) }.ok()?;
    let message = unsafe { &*message };
    let severity = message.Severity;
    let wanted =
        severity == D3D12_MESSAGE_SEVERITY_CORRUPTION || severity == D3D12_MESSAGE_SEVERITY_ERROR;
    let text =
        unsafe { std::slice::from_raw_parts(message.pDescription, message.DescriptionByteLength) };
    wanted.then(|| {
        String::from_utf8_lossy(text)
            .trim_end_matches('\0')
            .to_string()
    })
}

pub fn drain_errors(device: &ID3D12Device) -> Vec<String> {
    let Ok(queue) = device.cast::<ID3D12InfoQueue>() else {
        return Vec::new();
    };
    let count = unsafe { queue.GetNumStoredMessagesAllowedByRetrievalFilter() };
    let found = (0..count)
        .filter_map(|index| message_text(&queue, index))
        .collect();
    unsafe { queue.ClearStoredMessages() };
    found
}
