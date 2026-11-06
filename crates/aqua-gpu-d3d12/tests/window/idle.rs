use std::ffi::c_void;
use std::process::Command;
use std::time::{Duration, Instant};

use aqua_gpu_d3d12::hal::selection::AdapterPreference;
use aqua_gpu_d3d12::hal::{create_factory, list_adapters};
use aqua_gpu_d3d12::window::WindowBackend;
use aqua_graph::Extent;
use aqua_protocol::GpuBackend;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HMODULE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, GetThreadTimes, OpenThread, THREAD_QUERY_INFORMATION,
};

use crate::backend::{frame, open, options, scene};
use crate::test_window::TestWindow;

const CHILD: &str = "AQUA_IDLE_ADAPTER_LUID";
const RESULT: &str = "idle-busy-ms-per-s=";
const BUSY_LIMIT_MS_PER_S: f64 = 50.0;
const THREAD_QUERY_START_ADDRESS: u32 = 9;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationThread(
        thread: HANDLE,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

struct ThreadTime {
    id: u32,
    module: String,
    cpu_ticks: u64,
}

fn ticks(time: FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
}

fn start_module(thread: HANDLE) -> String {
    let mut start = 0usize;
    let queried = unsafe {
        NtQueryInformationThread(
            thread,
            THREAD_QUERY_START_ADDRESS,
            &mut start as *mut usize as *mut c_void,
            std::mem::size_of::<usize>() as u32,
            std::ptr::null_mut(),
        )
    } == 0;
    let mut module = HMODULE::default();
    let found = queried
        && unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                PCWSTR(start as *const u16),
                &mut module,
            )
        }
        .is_ok();
    match found {
        false => format!("{start:#x}"),
        true => {
            let mut name = [0u16; 260];
            let length = unsafe { GetModuleFileNameW(Some(module), &mut name) } as usize;
            let path = String::from_utf16_lossy(&name[..length]);
            path.rsplit('\\').next().unwrap_or_default().to_string()
        }
    }
}

fn thread_time(id: u32) -> Option<ThreadTime> {
    let thread = unsafe { OpenThread(THREAD_QUERY_INFORMATION, false, id) }.ok()?;
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    let timed =
        unsafe { GetThreadTimes(thread, &mut created, &mut exited, &mut kernel, &mut user) }
            .is_ok();
    let sample = timed.then(|| ThreadTime {
        id,
        module: start_module(thread),
        cpu_ticks: ticks(kernel) + ticks(user),
    });
    unsafe {
        let _ = CloseHandle(thread);
    }
    sample
}

fn process_threads() -> Vec<ThreadTime> {
    let process = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.expect("snapshot");
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut ids = Vec::new();
    let mut more = unsafe { Thread32First(snapshot, &mut entry) }.is_ok();
    while more {
        if entry.th32OwnerProcessID == process {
            ids.push(entry.th32ThreadID);
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    ids.into_iter().filter_map(thread_time).collect()
}

fn busy_threads(window: &TestWindow, span: Duration) -> Vec<(String, f64)> {
    let main = unsafe { GetCurrentThreadId() };
    let before = process_threads();
    let started = Instant::now();
    while started.elapsed() < span {
        window.pump();
        std::thread::sleep(Duration::from_millis(50));
    }
    let seconds = started.elapsed().as_secs_f64();
    process_threads()
        .into_iter()
        .filter(|after| after.id != main)
        .map(|after| {
            let previous = before
                .iter()
                .find(|sample| sample.id == after.id)
                .map_or(0, |sample| sample.cpu_ticks);
            let ms_per_s = (after.cpu_ticks - previous) as f64 / 10_000.0 / seconds;
            (after.module, ms_per_s)
        })
        .filter(|(_, ms_per_s)| *ms_per_s > 1.0)
        .collect()
}

fn start(luid: i64, early: bool) -> WindowBackend {
    let options = options(AdapterPreference::Luid(luid));
    match early {
        true => {
            WindowBackend::begin_early(options, || None);
            WindowBackend::take_early()
                .expect("early backend was started")
                .expect("early backend builds")
        }
        false => WindowBackend::new(options).expect("backend"),
    }
}

fn present_resize_and_idle(luid: i64, early: bool) {
    let mut backend = start(luid, early);
    let window = TestWindow::open(640, 480);
    let (id, extent) = open(&mut backend, &window);
    (0..10).for_each(|index| {
        frame(
            &mut backend,
            &window,
            id,
            &scene(extent, index as f32 / 10.0),
        );
        backend.finish_startup();
        if backend.startup_pending() {
            backend.startup_step();
        }
    });
    while backend.startup_pending() {
        backend.startup_step();
    }
    (0..3).for_each(|step| {
        window.resize(700 + 40 * step, 520 + 30 * step);
        let (width, height) = window.client();
        let extent = Extent { width, height };
        backend.resize_surface(id, extent).expect("resize");
        (0..5).for_each(|index| {
            frame(
                &mut backend,
                &window,
                id,
                &scene(extent, index as f32 / 5.0),
            )
        });
    });
    let busy = busy_threads(&window, Duration::from_secs(3));
    let total = busy.iter().fold(0.0, |sum, (_, ms_per_s)| sum + ms_per_s);
    println!("{RESULT}{total:.1} {busy:?}");
}

#[test]
fn idle_after_resize_stays_quiet_on_every_adapter() {
    quiet_on_every_adapter(
        "idle::idle_after_resize_stays_quiet_on_every_adapter",
        false,
    );
}

#[test]
fn idle_after_chunked_startup_stays_quiet_on_every_adapter() {
    quiet_on_every_adapter(
        "idle::idle_after_chunked_startup_stays_quiet_on_every_adapter",
        true,
    );
}

fn quiet_on_every_adapter(name: &str, early: bool) {
    if let Ok(luid) = std::env::var(CHILD) {
        return present_resize_and_idle(luid.parse().expect("luid"), early);
    }
    let factory = create_factory(false).expect("factory");
    let exe = std::env::current_exe().expect("test exe");
    let busy: Vec<String> = list_adapters(&factory)
        .into_iter()
        .filter_map(|adapter| {
            let output = Command::new(&exe)
                .args(["--exact", name, "--nocapture"])
                .env(CHILD, adapter.luid.to_string())
                .output()
                .expect("child");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let line = stdout
                .lines()
                .find_map(|line| {
                    line.find(RESULT)
                        .map(|at| line[at + RESULT.len()..].to_string())
                })
                .unwrap_or_else(|| panic!("{} produced no result: {stdout}", adapter.name));
            eprintln!("[{}] idle {line}", adapter.name);
            let total: f64 = line
                .split(' ')
                .next()
                .and_then(|value| value.parse().ok())
                .expect("busy total");
            (total >= BUSY_LIMIT_MS_PER_S).then(|| format!("{}: {line}", adapter.name))
        })
        .collect();
    assert!(busy.is_empty(), "threads stay busy after resize: {busy:?}");
}
