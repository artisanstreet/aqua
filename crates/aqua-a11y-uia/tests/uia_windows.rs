#![cfg(windows)]

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use aqua_a11y_uia::{Connection, UiaAdapter};
use aqua_core::{NodeId, RawId, Rect, Role, SemanticNode};
use aqua_protocol::{A11yAction, A11yActionRequest, A11yNode, AccessTreeUpdate};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationInvokePattern, TreeScope_Descendants,
    UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_InvokePatternId, UIA_TextControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, MSG, PostMessageW, SW_SHOWNA,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WS_POPUP,
};
use windows::core::w;

const WM_SET_VERSION: u32 = WM_APP + 1;
const WM_STOP: u32 = WM_APP + 2;

fn id(index: u32) -> NodeId {
    NodeId::from_raw(RawId {
        index,
        generation: 0,
    })
}

fn item(index: u32, role: Role, label: &str, children: Vec<NodeId>) -> A11yNode {
    let mut semantic = SemanticNode::new(id(index), role);
    semantic.label = label.into();
    A11yNode {
        semantic,
        bounds: Some(Rect {
            x: 10.0,
            y: 10.0 + index as f32 * 30.0,
            w: 120.0,
            h: 24.0,
        }),
        children,
        text: None,
    }
}

fn tree(version: usize) -> AccessTreeUpdate {
    let mut checkbox = item(3, Role::Checkbox, "Enable logging", vec![]);
    checkbox.semantic.checked = Some(version > 1);
    let run_label = if version > 1 { "Run again" } else { "Run" };
    AccessTreeUpdate {
        root: id(0),
        focus: id(1),
        nodes: vec![
            item(0, Role::Group, "Aqua test", vec![id(1), id(2), id(3)]),
            item(1, Role::Button, run_label, vec![]),
            item(2, Role::Text, "Status ready", vec![]),
            checkbox,
        ],
    }
}

struct Host {
    hwnd: isize,
    initial: Connection,
    actions: mpsc::Receiver<A11yActionRequest>,
    join: thread::JoinHandle<()>,
}

fn spawn_host() -> Host {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (action_tx, action_rx) = mpsc::channel();
    let join = thread::spawn(move || {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("aqua uia conformance"),
                WS_POPUP,
                -32000,
                -32000,
                400,
                300,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let raw = hwnd.0 as isize;
        let waker_target = raw;
        let waker = Arc::new(move || {
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(waker_target as *mut _)),
                    WM_APP,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
        });
        let mut adapter = UiaAdapter::attach(raw, waker).unwrap();
        ready_tx.send((raw, adapter.connection())).unwrap();
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }
        let mut version = 1;
        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
            match message.message {
                WM_STOP => break,
                WM_SET_VERSION => {
                    version = message.wParam.0;
                    adapter.apply(&tree(version));
                }
                WM_APP => {
                    adapter.apply(&tree(version));
                    for action in adapter.drain_actions() {
                        action_tx.send(action).unwrap();
                    }
                }
                _ => unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                },
            }
        }
        drop(adapter);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    });
    let (hwnd, initial) = ready_rx.recv().unwrap();
    Host {
        hwnd,
        initial,
        actions: action_rx,
        join,
    }
}

fn automation() -> IUIAutomation {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).unwrap()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    name: String,
    control_type: i32,
}

fn descendants(
    automation: &IUIAutomation,
    hwnd: isize,
) -> Vec<(
    Seen,
    windows::Win32::UI::Accessibility::IUIAutomationElement,
)> {
    unsafe {
        let root = automation.ElementFromHandle(HWND(hwnd as *mut _)).unwrap();
        let all = root
            .FindAll(
                TreeScope_Descendants,
                &automation.CreateTrueCondition().unwrap(),
            )
            .unwrap();
        (0..all.Length().unwrap())
            .map(|index| {
                let element = all.GetElement(index).unwrap();
                let seen = Seen {
                    name: element.CurrentName().unwrap().to_string(),
                    control_type: element.CurrentControlType().unwrap().0,
                };
                (seen, element)
            })
            .collect()
    }
}

fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for UIA state");
        thread::sleep(Duration::from_millis(50));
    }
}

fn stop(host: Host) {
    unsafe {
        PostMessageW(
            Some(HWND(host.hwnd as *mut _)),
            WM_STOP,
            WPARAM(0),
            LPARAM(0),
        )
        .unwrap();
    }
    host.join.join().unwrap();
}

#[test]
fn uia_client_sees_the_tree_invokes_actions_and_observes_live_updates() {
    let host = spawn_host();
    assert_eq!(host.initial, Connection::Dormant);
    let client = automation();

    let seen = wait_for(|| {
        let found = descendants(&client, host.hwnd);
        found
            .iter()
            .any(|(seen, _)| seen.name == "Run")
            .then_some(found)
    });
    let kinds: Vec<_> = seen
        .iter()
        .map(|(seen, _)| (seen.name.as_str(), seen.control_type))
        .collect();
    assert!(kinds.contains(&("Run", UIA_ButtonControlTypeId.0)));
    assert!(kinds.contains(&("Enable logging", UIA_CheckBoxControlTypeId.0)));
    assert!(
        kinds.contains(&("Status ready", UIA_TextControlTypeId.0)),
        "{kinds:?}"
    );

    let button = &seen.iter().find(|(seen, _)| seen.name == "Run").unwrap().1;
    let rect = unsafe { button.CurrentBoundingRectangle() }.unwrap();
    assert!(rect.right > rect.left && rect.bottom > rect.top);

    let invoke: IUIAutomationInvokePattern =
        unsafe { button.GetCurrentPatternAs(UIA_InvokePatternId) }.unwrap();
    unsafe { invoke.Invoke() }.unwrap();
    let mut received = Vec::new();
    wait_for(|| {
        received.extend(host.actions.try_iter());
        received
            .iter()
            .any(|action| action.node == id(1) && action.action == A11yAction::Press)
            .then_some(())
    });

    unsafe {
        PostMessageW(
            Some(HWND(host.hwnd as *mut _)),
            WM_SET_VERSION,
            WPARAM(2),
            LPARAM(0),
        )
        .unwrap();
    }
    wait_for(|| {
        descendants(&client, host.hwnd)
            .iter()
            .any(|(seen, _)| seen.name == "Run again")
            .then_some(())
    });
    stop(host);
}
