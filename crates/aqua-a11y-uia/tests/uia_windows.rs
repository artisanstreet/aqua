#![cfg(windows)]

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use std::cell::{Cell, RefCell};

use aqua_a11y_uia::{Connection, UiaAdapter, UiaRouter};
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
    CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationRangeValuePattern, IUIAutomationTextPattern, IUIAutomationTextRange,
    IUIAutomationTogglePattern, IUIAutomationValuePattern, TextPatternRangeEndpoint_End,
    TextPatternRangeEndpoint_Start, TextUnit_Character, ToggleState_Off, ToggleState_On,
    TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_InvokePatternId,
    UIA_RangeValuePatternId, UIA_TextControlTypeId, UIA_TextPatternId, UIA_TogglePatternId,
    UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    SetWindowLongPtrW, ShowWindow, TranslateMessage, GWLP_WNDPROC, MSG, SW_SHOWNA, WINDOW_EX_STYLE,
    WM_APP, WNDPROC, WS_POPUP,
};

thread_local! {
    static ROUTER: RefCell<Option<UiaRouter>> = const { RefCell::new(None) };
    static PREVIOUS: Cell<isize> = const { Cell::new(0) };
}

extern "system" fn routed_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    let routed = ROUTER
        .with(|router| {
            router
                .borrow()
                .as_ref()
                .and_then(|router| router.handle(message, wparam.0, lparam.0))
        })
        .map(windows::Win32::Foundation::LRESULT);
    match routed {
        Some(result) => result,
        None => {
            let previous: WNDPROC = unsafe { std::mem::transmute(PREVIOUS.get()) };
            unsafe { CallWindowProcW(previous, hwnd, message, wparam, lparam) }
        }
    }
}

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
    let mut locked = item(6, Role::Button, "Locked", vec![]);
    locked.semantic.disabled = true;
    AccessTreeUpdate {
        root: id(0),
        focus: id(1),
        nodes: vec![
            item(
                0,
                Role::Group,
                "Aqua test",
                vec![id(1), id(2), id(3), id(4), id(5), id(6)],
            ),
            item(1, Role::Button, run_label, vec![]),
            item(2, Role::Text, "Status ready", vec![]),
            checkbox,
            field,
            slider,
            locked,
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
        let (mut adapter, router) = UiaAdapter::attach_routed(raw, waker).unwrap();
        ROUTER.with(|slot| *slot.borrow_mut() = Some(router));
        PREVIOUS.set(unsafe {
            SetWindowLongPtrW(hwnd, GWLP_WNDPROC, routed_proc as *const () as isize)
        });
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

fn collect_actions(host: &Host, received: &mut Vec<A11yActionRequest>) {
    received.extend(host.actions.try_iter());
}

#[test]
fn uia_exposes_hierarchy_focusability_and_enabled_state() {
    let host = spawn_host();
    let client = automation();
    let button = element_named(&client, &host, "Run");
    let locked = element_named(&client, &host, "Locked");
    let status = element_named(&client, &host, "Status ready");

    let walker = unsafe { client.ControlViewWalker() }.unwrap();
    let parent: IUIAutomationElement = unsafe { walker.GetParentElement(&button) }.unwrap();
    assert_eq!(
        unsafe { parent.CurrentName() }.unwrap().to_string(),
        "Aqua test"
    );
    let first = unsafe { walker.GetFirstChildElement(&parent) }.unwrap();
    assert_eq!(unsafe { first.CurrentName() }.unwrap().to_string(), "Run");
    let second = unsafe { walker.GetNextSiblingElement(&first) }.unwrap();
    assert_eq!(
        unsafe { second.CurrentName() }.unwrap().to_string(),
        "Status ready"
    );
    let last = unsafe { walker.GetLastChildElement(&parent) }.unwrap();
    assert_eq!(unsafe { last.CurrentName() }.unwrap().to_string(), "Locked");
    let children = unsafe {
        parent.FindAll(
            windows::Win32::UI::Accessibility::TreeScope_Children,
            &client.CreateTrueCondition().unwrap(),
        )
    }
    .unwrap();
    assert_eq!(unsafe { children.Length() }.unwrap(), 6);

    assert!(unsafe { button.CurrentIsEnabled() }.unwrap().as_bool());
    assert!(!unsafe { locked.CurrentIsEnabled() }.unwrap().as_bool());
    assert!(unsafe { button.CurrentIsKeyboardFocusable() }
        .unwrap()
        .as_bool());
    assert!(!unsafe { status.CurrentIsKeyboardFocusable() }
        .unwrap()
        .as_bool());

    let field = element_named(&client, &host, "Name");
    unsafe { field.SetFocus() }.unwrap();
    let mut received = Vec::new();
    wait_for(|| {
        collect_actions(&host, &mut received);
        received
            .iter()
            .any(|action| action.node == id(4) && action.action == A11yAction::Focus)
            .then_some(())
    });
    stop(host);
}

fn text_of(range: &IUIAutomationTextRange) -> String {
    unsafe { range.GetText(-1) }.unwrap().to_string()
}

#[test]
fn uia_text_ranges_move_by_character_over_multibyte_text_and_select() {
    let host = spawn_host();
    let client = automation();
    let field = element_named(&client, &host, "Name");
    let text: IUIAutomationTextPattern =
        unsafe { field.GetCurrentPatternAs(UIA_TextPatternId) }.unwrap();
    let document = unsafe { text.DocumentRange() }.unwrap();
    let range = unsafe { document.Clone() }.unwrap();
    unsafe {
        range.MoveEndpointByRange(
            TextPatternRangeEndpoint_End,
            &range,
            TextPatternRangeEndpoint_Start,
        )
    }
    .unwrap();
    assert_eq!(text_of(&range), "");
    let moved = unsafe { range.Move(TextUnit_Character, 1) }.unwrap();
    assert_eq!(moved, 1);
    unsafe { range.ExpandToEnclosingUnit(TextUnit_Character) }.unwrap();
    assert_eq!(text_of(&range), "é");
    let moved = unsafe { range.Move(TextUnit_Character, 3) }.unwrap();
    assert_eq!(moved, 3);
    unsafe { range.ExpandToEnclosingUnit(TextUnit_Character) }.unwrap();
    assert_eq!(text_of(&range), "o");
    let past = unsafe { range.Move(TextUnit_Character, 4) }.unwrap();
    assert_eq!(past, 1);

    let accent = unsafe { document.Clone() }.unwrap();
    unsafe {
        accent.MoveEndpointByRange(
            TextPatternRangeEndpoint_End,
            &accent,
            TextPatternRangeEndpoint_Start,
        )
    }
    .unwrap();
    unsafe { accent.Move(TextUnit_Character, 1) }.unwrap();
    unsafe { accent.ExpandToEnclosingUnit(TextUnit_Character) }.unwrap();
    unsafe { accent.Select() }.unwrap();
    let mut received = Vec::new();
    wait_for(|| {
        collect_actions(&host, &mut received);
        received
            .iter()
            .any(|action| {
                action.node == id(4)
                    && matches!(
                        action.data,
                        A11yActionData::TextSelection { .. }
                            | A11yActionData::TextByteSelection { .. }
                    )
            })
            .then_some(())
    });
    let selection = received
        .iter()
        .find_map(|action| match &action.data {
            A11yActionData::TextByteSelection {
                anchor_byte,
                focus_byte,
                ..
            } => Some((*anchor_byte, *focus_byte)),
            _ => None,
        })
        .expect("byte selection");
    assert_eq!(selection, (1, 3));
    stop(host);
}
