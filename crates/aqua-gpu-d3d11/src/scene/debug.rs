use windows::Win32::Graphics::Direct3D11::*;
use windows::core::Interface;

pub fn requested() -> bool {
    std::env::var("AQUA_D3D11_DEBUG").as_deref() == Ok("1")
}

fn message_text(queue: &ID3D11InfoQueue, index: u64) -> Option<String> {
    let mut length = 0usize;
    unsafe { queue.GetMessage(index, None, &mut length) }.ok()?;
    let mut storage = vec![0u64; length.div_ceil(8)];
    let message = storage.as_mut_ptr() as *mut D3D11_MESSAGE;
    unsafe { queue.GetMessage(index, Some(message), &mut length) }.ok()?;
    let message = unsafe { &*message };
    let wanted = message.Severity == D3D11_MESSAGE_SEVERITY_CORRUPTION
        || message.Severity == D3D11_MESSAGE_SEVERITY_ERROR
        || message.Severity == D3D11_MESSAGE_SEVERITY_WARNING;
    let text =
        unsafe { std::slice::from_raw_parts(message.pDescription, message.DescriptionByteLength) };
    wanted.then(|| {
        format!(
            "{:?} #{}: {}",
            message.Severity,
            message.ID.0,
            String::from_utf8_lossy(text).trim_end_matches('\0')
        )
    })
}

pub fn active(device: &ID3D11Device) -> bool {
    device.cast::<ID3D11InfoQueue>().is_ok()
}

pub fn drain(device: &ID3D11Device) -> Vec<String> {
    let Ok(queue) = device.cast::<ID3D11InfoQueue>() else {
        return Vec::new();
    };
    let count = unsafe { queue.GetNumStoredMessagesAllowedByRetrievalFilter() };
    let found = (0..count)
        .filter_map(|index| message_text(&queue, index))
        .collect();
    unsafe { queue.ClearStoredMessages() };
    found
}
