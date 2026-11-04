#![cfg(windows)]

use aqua_core::InputEvent;
use aqua_protocol::{Instant, WindowId, WindowMaterial};
use aqua_shell_win32::{
    now, quit, run, set_message_hook, FileDrag, Handler, MessageHook, RawMessage, ShellEvent,
    Window, WindowOptions,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINTL, RECT, WPARAM};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{IDropTarget, DROPEFFECT, DROPEFFECT_COPY};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::WindowsAndMessaging::{
    GetPropW, PostMessageW, SendMessageW, WM_APP, WM_DPICHANGED, WM_POINTERDOWN, WM_POINTERUPDATE,
};

type Log = Rc<RefCell<Vec<(WindowId, ShellEvent)>>>;
type Step = Box<dyn FnMut(&Log) -> bool>;

struct Script {
    log: Log,
    steps: std::vec::IntoIter<Step>,
    current: Option<Step>,
    deadline: Instant,
}

impl Handler for Script {
    fn window_event(&mut self, id: WindowId, event: ShellEvent) {
        self.log.borrow_mut().push((id, event));
    }

    fn tick(&mut self, at: Instant) -> Option<Instant> {
        assert!(at < self.deadline, "script timed out");
        loop {
            let step = match self.current.as_mut() {
                Some(step) => step,
                None => match self.steps.next() {
                    Some(next) => self.current.insert(next),
                    None => {
                        quit();
                        return None;
                    }
                },
            };
            match step(&self.log) {
                true => self.current = None,
                false => return Some(at + Duration::from_millis(5)),
            }
        }
    }
}

fn drive(log: Log, steps: Vec<Step>) {
    let script = Script {
        log,
        steps: steps.into_iter(),
        current: None,
        deadline: now() + Duration::from_secs(20),
    };
    run(script).expect("loop");
}

fn hidden() -> WindowOptions {
    WindowOptions {
        title: "aqua-shell-test".into(),
        logical_size: (320.0, 200.0),
        visible: false,
        ..Default::default()
    }
}

fn hwnd(window: Window) -> HWND {
    HWND(window.raw_hwnd() as *mut _)
}

fn events_of(log: &Log) -> Vec<ShellEvent> {
    log.borrow().iter().map(|(_, e)| e.clone()).collect()
}

#[test]
fn runtime_dpi_change_updates_scale_regions_and_size() {
    let log: Log = Default::default();
    let window = Window::create(&hidden()).expect("window");
    let before = window.inner_size();
    let before_scale = window.scale();
    let target_dpi = 192u32;
    let target_scale = f64::from(target_dpi) / 96.0;
    let suggested = RECT {
        left: 100,
        top: 100,
        right: 100 + (before.0 as f64 / before_scale * target_scale) as i32 + 16,
        bottom: 100 + (before.1 as f64 / before_scale * target_scale) as i32 + 39,
    };
    let sent = Box::new(RefCell::new(false));
    let probe = log.clone();
    drive(
        log.clone(),
        vec![
            Box::new(move |_| {
                let mut sent = sent.borrow_mut();
                if !*sent {
                    *sent = true;
                    unsafe {
                        SendMessageW(
                            hwnd(window),
                            WM_DPICHANGED,
                            Some(WPARAM(target_dpi as usize | ((target_dpi as usize) << 16))),
                            Some(LPARAM(&suggested as *const RECT as isize)),
                        )
                    };
                }
                true
            }),
            Box::new(move |_| {
                let log = probe.borrow();
                let at = log.iter().position(|(_, e)| {
                    *e == ShellEvent::ScaleChanged {
                        scale: target_scale,
                    }
                });
                at.is_some_and(|at| {
                    log[at..]
                        .iter()
                        .any(|(_, e)| matches!(e, ShellEvent::Resized { .. }))
                })
            }),
        ],
    );
    let events = events_of(&log);
    let scale_at = events
        .iter()
        .position(|e| {
            *e == ShellEvent::ScaleChanged {
                scale: target_scale,
            }
        })
        .expect("scale event");
    let resize_after = events[scale_at..]
        .iter()
        .any(|e| matches!(e, ShellEvent::Resized { .. }));
    assert!(resize_after, "resize must follow the scale change");
    let (w, h) = window.inner_size();
    assert!(w > before.0 && h > before.1, "{:?} -> {:?}", before, (w, h));
    window.destroy();
}

struct Pump {
    seen: Rc<RefCell<Vec<u32>>>,
    swallow: u32,
    reply: u32,
}

impl MessageHook for Pump {
    fn pre_translate(&mut self, message: &RawMessage) -> bool {
        self.seen.borrow_mut().push(message.message);
        message.message == self.swallow
    }

    fn window_message(&mut self, message: &RawMessage) -> Option<isize> {
        (message.message == self.reply).then_some(77)
    }
}

#[test]
fn message_hook_sees_every_message_before_translation_and_can_answer_for_windows() {
    let window = Window::create(&hidden()).expect("window");
    let seen = Rc::new(RefCell::new(Vec::new()));
    let swallowed = WM_APP + 40;
    let answered = WM_APP + 41;
    set_message_hook(Pump {
        seen: seen.clone(),
        swallow: swallowed,
        reply: answered,
    });
    let answer = Rc::new(RefCell::new(None));
    let slot = answer.clone();
    let posted = Rc::new(RefCell::new(false));
    drive(
        Default::default(),
        vec![
            Box::new(move |_| {
                if !*posted.borrow() {
                    *posted.borrow_mut() = true;
                    unsafe {
                        let _ = PostMessageW(Some(hwnd(window)), swallowed, WPARAM(0), LPARAM(0));
                        let _ = PostMessageW(Some(hwnd(window)), answered, WPARAM(0), LPARAM(0));
                    }
                }
                true
            }),
            Box::new(move |_| {
                let result = unsafe { SendMessageW(hwnd(window), answered, None, None) };
                *slot.borrow_mut() = Some(result.0);
                true
            }),
            Box::new({
                let seen = seen.clone();
                move |_| seen.borrow().contains(&swallowed) && seen.borrow().contains(&answered)
            }),
        ],
    );
    assert_eq!(*answer.borrow(), Some(77));
    window.destroy();
}

#[test]
fn bogus_pointer_ids_fall_through_without_input_events() {
    let log: Log = Default::default();
    let window = Window::create(&hidden()).expect("window");
    let sent = Rc::new(RefCell::new(false));
    drive(
        log.clone(),
        vec![Box::new(move |_| {
            if !*sent.borrow() {
                *sent.borrow_mut() = true;
                unsafe {
                    SendMessageW(hwnd(window), WM_POINTERDOWN, Some(WPARAM(0xFFF0)), None);
                    SendMessageW(hwnd(window), WM_POINTERUPDATE, Some(WPARAM(0xFFF0)), None);
                }
            }
            true
        })],
    );
    assert!(
        !events_of(&log).iter().any(|e| matches!(
            e,
            ShellEvent::Input(InputEvent::PointerDown(..) | InputEvent::PointerMove(..))
        )),
        "{:?}",
        events_of(&log)
    );
    window.destroy();
}

fn file_data_object(path: &std::path::Path) -> IDataObject {
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
    };
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None) }.expect("pidl");
    let items =
        unsafe { SHCreateShellItemArrayFromIDLists(&[pidl as *const ITEMIDLIST]) }.expect("items");
    unsafe { items.BindToHandler(None, &BHID_DataObject) }.expect("data")
}

#[test]
fn file_drag_and_drop_reaches_the_handler_in_logical_coordinates() {
    let log: Log = Default::default();
    let window = Window::create(&hidden()).expect("window");
    let scale = window.scale();
    let exe = std::env::temp_dir().join("aqua-shell-drop-probe.txt");
    std::fs::write(&exe, b"probe").expect("probe file");
    let object = file_data_object(&exe);
    let raw = unsafe { GetPropW(hwnd(window), w!("OleDropTargetInterface")) };
    assert!(!raw.0.is_null(), "no drop target registered");
    let target: IDropTarget = unsafe { IDropTarget::from_raw_borrowed(&raw.0) }
        .expect("target")
        .clone();
    let (left, top, _, _) = window.outer_rect();
    let origin = {
        let mut p = windows::Win32::Foundation::POINT::default();
        let _ = unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(hwnd(window), &mut p) };
        p
    };
    let _ = (left, top);
    let at = POINTL {
        x: origin.x + (100.0 * scale) as i32,
        y: origin.y + (50.0 * scale) as i32,
    };
    let done = Rc::new(RefCell::new(false));
    drive(
        log.clone(),
        vec![
            Box::new({
                let target = target.clone();
                let object = object.clone();
                let done = done.clone();
                move |_| {
                    if *done.borrow() {
                        return true;
                    }
                    *done.borrow_mut() = true;
                    let mut effect = DROPEFFECT(0);
                    unsafe {
                        target
                            .DragEnter(&object, MODIFIERKEYS_FLAGS(0), at, &mut effect)
                            .expect("enter");
                        assert_eq!(effect, DROPEFFECT_COPY);
                        target
                            .DragOver(MODIFIERKEYS_FLAGS(0), at, &mut effect)
                            .expect("over");
                        target
                            .Drop(&object, MODIFIERKEYS_FLAGS(0), at, &mut effect)
                            .expect("drop");
                    }
                    true
                }
            }),
            Box::new({
                let log = log.clone();
                move |_| {
                    log.borrow()
                        .iter()
                        .any(|(_, e)| matches!(e, ShellEvent::FileDrag(FileDrag::Drop { .. })))
                }
            }),
        ],
    );
    let drags: Vec<FileDrag> = events_of(&log)
        .into_iter()
        .filter_map(|e| match e {
            ShellEvent::FileDrag(d) => Some(d),
            _ => None,
        })
        .collect();
    assert!(matches!(drags[0], FileDrag::Enter { .. }), "{drags:?}");
    assert!(matches!(drags[1], FileDrag::Over { .. }));
    match (&drags[0], &drags[2]) {
        (
            FileDrag::Enter {
                paths: entered,
                position,
            },
            FileDrag::Drop {
                paths: dropped,
                position: end,
            },
        ) => {
            assert_eq!(entered.len(), 1);
            assert_eq!(
                entered[0].file_name().map(|n| n.to_ascii_lowercase()),
                exe.file_name().map(|n| n.to_ascii_lowercase())
            );
            assert_eq!(entered, dropped);
            assert!(
                (position.x - 100.0).abs() < 1.0 && (position.y - 50.0).abs() < 1.0,
                "{position:?}"
            );
            assert_eq!(position, end);
        }
        other => panic!("{other:?}"),
    }
    window.destroy();
}

#[test]
fn composition_swapchain_attaches_to_an_alpha_window_with_a_backdrop() {
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        IDXGIDevice, IDXGIFactory2, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
        DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    };
    let window = Window::create(&WindowOptions {
        alpha_surface: true,
        material: WindowMaterial::Mica,
        dark: true,
        ..hidden()
    })
    .expect("window");
    assert!(window.has_alpha_surface());
    let mut device: Option<ID3D11Device> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    }
    .expect("d3d11");
    let device = device.expect("device");
    let dxgi: IDXGIDevice = device.cast().expect("dxgi");
    let factory: IDXGIFactory2 = unsafe {
        dxgi.GetAdapter()
            .expect("adapter")
            .GetParent()
            .expect("factory")
    };
    let (width, height) = window.inner_size();
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: width,
        Height: height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        ..Default::default()
    };
    let chain =
        unsafe { factory.CreateSwapChainForComposition(&device, &desc, None) }.expect("swapchain");
    unsafe { chain.Present(0, Default::default()) }
        .ok()
        .expect("present");
    let surface: windows::core::IUnknown = chain.cast().expect("unknown");
    window.attach_composition_surface(&surface).expect("attach");
    window
        .attach_composition_surface(&surface)
        .expect("attach again");
    let outcome = window.set_material(WindowMaterial::Mica);
    assert!(
        matches!(outcome.active, WindowMaterial::Mica | WindowMaterial::Solid),
        "{outcome:?}"
    );
    window.destroy();
}
