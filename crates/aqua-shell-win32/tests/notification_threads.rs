#![cfg(windows)]

use aqua_shell_win32::notifications::{AppIdentity, NotificationCenter};
use std::collections::HashSet;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::Threading::GetCurrentProcessId;

fn thread_ids() -> HashSet<u32> {
    let process = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.expect("snapshot");
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut ids = HashSet::new();
    let mut more = unsafe { Thread32First(snapshot, &mut entry) }.is_ok();
    while more {
        if entry.th32OwnerProcessID == process {
            ids.insert(entry.th32ThreadID);
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) }.is_ok();
    }
    ids
}

#[test]
fn creating_a_notification_center_and_withdrawing_before_any_toast_starts_no_threads() {
    let before = thread_ids();
    let mut center = NotificationCenter::new(
        AppIdentity::new("com.artisanstreet.thread-budget", "Thread budget").expect("identity"),
        std::sync::Arc::new(|_| {}),
    )
    .expect("center");
    let _ = center.withdraw("never-shown");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let started: Vec<u32> = thread_ids().difference(&before).copied().collect();
    assert!(started.is_empty(), "threads started: {started:?}");
}
