#![cfg(windows)]

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use aqua_a11y_uia::{Connection, UiaAdapter};
use aqua_core::NumericRangeSemantics;
use aqua_core::{NodeId, RawId, Rect, Role, SemanticNode};
use aqua_protocol::{
    A11yAction, A11yActionData, A11yActionRequest, A11yNode, A11yText, A11yTextRun,
    AccessTreeUpdate,
};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationInvokePattern, IUIAutomationRangeValuePattern,
    IUIAutomationTextPattern, IUIAutomationTogglePattern, IUIAutomationValuePattern,
    ToggleState_Off, ToggleState_On, TreeScope_Descendants, UIA_ButtonControlTypeId,
    UIA_CheckBoxControlTypeId, UIA_InvokePatternId, UIA_RangeValuePatternId, UIA_TextControlTypeId,
    UIA_TextPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW, ShowWindow,
    TranslateMessage, MSG, SW_SHOWNA, WINDOW_EX_STYLE, WM_APP, WS_POPUP,
};

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
    let mut field = item(4, Role::TextField, "Name", vec![]);
    field.semantic.value_text = Some("héllo".into());
    field.text = Some(A11yText {
        selection_enabled: true,
        selection: Some((0, 0)),
        runs: vec![A11yTextRun {
            text: "héllo".into(),
            byte_start: 0,
            bounds: Rect {
                x: 10.0,
                y: 130.0,
                w: 100.0,
                h: 24.0,
            },
            word_starts: vec![0],
            character_lengths: vec![1, 2, 1, 1, 1],
            character_positions: vec![0.0, 10.0, 20.0, 30.0, 40.0],
            character_widths: vec![10.0; 5],
            rtl: false,
        }],
    });
    let mut slider = item(5, Role::Slider, "Volume", vec![]);
    slider.semantic.numeric_range = Some(NumericRangeSemantics {
        min: 0.0,
        max: 10.0,
        value: 4.0,
        step: 1.0,
    });
    AccessTreeUpdate {
        root: id(0),
        focus: id(1),
        nodes: vec![
            item(
                0,
                Role::Group,
                "Aqua test",
                vec![id(1), id(2), id(3), id(4), id(5)],
            ),
            item(1, Role::Button, run_label, vec![]),
            item(2, Role::Text, "Status ready", vec![]),
            checkbox,
            field,
            slider,
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

fn element_named(
    client: &IUIAutomation,
    host: &Host,
    name: &str,
) -> windows::Win32::UI::Accessibility::IUIAutomationElement {
    wait_for(|| {
        descendants(client, host.hwnd)
            .into_iter()
            .find(|(seen, _)| seen.name == name)
            .map(|(_, element)| element)
    })
}

fn post_version(host: &Host, version: usize) {
    unsafe {
        PostMessageW(
            Some(HWND(host.hwnd as *mut _)),
            WM_SET_VERSION,
            WPARAM(version),
            LPARAM(0),
        )
        .unwrap();
    }
}

#[test]
fn uia_toggle_value_range_and_text_patterns_work() {
    let host = spawn_host();
    let client = automation();

    let checkbox = element_named(&client, &host, "Enable logging");
    let toggle: IUIAutomationTogglePattern =
        unsafe { checkbox.GetCurrentPatternAs(UIA_TogglePatternId) }.unwrap();
    assert_eq!(
        unsafe { toggle.CurrentToggleState() }.unwrap(),
        ToggleState_Off
    );
    post_version(&host, 2);
    wait_for(|| {
        (unsafe { toggle.CurrentToggleState() }.ok() == Some(ToggleState_On)).then_some(())
    });
    unsafe { toggle.Toggle() }.unwrap();
    let mut received = Vec::new();
    wait_for(|| {
        received.extend(host.actions.try_iter());
        received
            .iter()
            .any(|action| action.node == id(3) && action.action == A11yAction::Press)
            .then_some(())
    });

    let field = element_named(&client, &host, "Name");
    let value: IUIAutomationValuePattern =
        unsafe { field.GetCurrentPatternAs(UIA_ValuePatternId) }.unwrap();
    assert_eq!(
        unsafe { value.CurrentValue() }.unwrap().to_string(),
        "héllo"
    );
    unsafe { value.SetValue(&"wörld".into()) }.unwrap();
    wait_for(|| {
        received.extend(host.actions.try_iter());
        received
            .iter()
            .any(|action| {
                action.node == id(4)
                    && action.action == A11yAction::SetValue
                    && action.data == A11yActionData::Value("wörld".into())
            })
            .then_some(())
    });

    let text: IUIAutomationTextPattern =
        unsafe { field.GetCurrentPatternAs(UIA_TextPatternId) }.unwrap();
    let range = unsafe { text.DocumentRange() }.unwrap();
    assert_eq!(unsafe { range.GetText(-1) }.unwrap().to_string(), "héllo");

    let slider = element_named(&client, &host, "Volume");
    let range_value: IUIAutomationRangeValuePattern =
        unsafe { slider.GetCurrentPatternAs(UIA_RangeValuePatternId) }.unwrap();
    assert_eq!(unsafe { range_value.CurrentValue() }.unwrap(), 4.0);
    assert_eq!(unsafe { range_value.CurrentMaximum() }.unwrap(), 10.0);
    unsafe { range_value.SetValue(7.0) }.unwrap();
    wait_for(|| {
        received.extend(host.actions.try_iter());
        received
            .iter()
            .any(|action| {
                action.node == id(5)
                    && action.action == A11yAction::SetValue
                    && action.data == A11yActionData::Numeric(7.0)
            })
            .then_some(())
    });
    stop(host);
}
