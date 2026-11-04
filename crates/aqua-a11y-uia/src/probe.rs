use crate::UiaAdapter;
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationValuePattern, TreeScope_Descendants, UIA_ButtonControlTypeId,
    UIA_CheckBoxControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_GroupControlTypeId, UIA_InvokePatternId, UIA_ListControlTypeId, UIA_ListItemControlTypeId,
    UIA_TextControlTypeId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW, ShowWindow,
    TranslateMessage, MSG, SW_SHOWNA, WINDOW_EX_STYLE, WM_APP, WS_POPUP,
};

const WM_STOP: u32 = WM_APP + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    Button,
    CheckBox,
    Edit,
    Text,
    List,
    ListItem,
    Group,
    Document,
    Other(i32),
}

impl ProbeKind {
    fn from_control_type(id: i32) -> Self {
        [
            (UIA_ButtonControlTypeId.0, Self::Button),
            (UIA_CheckBoxControlTypeId.0, Self::CheckBox),
            (UIA_EditControlTypeId.0, Self::Edit),
            (UIA_TextControlTypeId.0, Self::Text),
            (UIA_ListControlTypeId.0, Self::List),
            (UIA_ListItemControlTypeId.0, Self::ListItem),
            (UIA_GroupControlTypeId.0, Self::Group),
            (UIA_DocumentControlTypeId.0, Self::Document),
        ]
        .into_iter()
        .find_map(|(known, kind)| (known == id).then_some(kind))
        .unwrap_or(Self::Other(id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeElement {
    pub name: String,
    pub kind: ProbeKind,
    pub bounds: (i32, i32, i32, i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    NotFound(String),
    Native(String),
    TimedOut,
}

pub struct UiaProbe {
    hwnd: isize,
    join: Option<JoinHandle<()>>,
    actions: Receiver<A11yActionRequest>,
    client: IUIAutomation,
}

fn native(error: windows::core::Error) -> ProbeError {
    ProbeError::Native(error.to_string())
}

impl UiaProbe {
    pub fn spawn(update: AccessTreeUpdate) -> Result<Self, ProbeError> {
        let (ready, ready_receiver) = channel();
        let (action_sender, actions) = channel();
        let join = std::thread::spawn(move || {
            let created = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("aqua uia probe"),
                    WS_POPUP,
                    -32000,
                    -32000,
                    1280,
                    800,
                    None,
                    None,
                    None,
                    None,
                )
            };
            let Ok(hwnd) = created else {
                let _ = ready.send(Err(()));
                return;
            };
            let raw = hwnd.0 as isize;
            let waker = Arc::new(move || {
                let _ = unsafe {
                    PostMessageW(Some(HWND(raw as *mut _)), WM_APP, WPARAM(0), LPARAM(0))
                };
            });
            let Ok(mut adapter) = UiaAdapter::attach(raw, waker) else {
                let _ = unsafe { DestroyWindow(hwnd) };
                let _ = ready.send(Err(()));
                return;
            };
            let _ = ready.send(Ok(raw));
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNA) };
            let mut message = MSG::default();
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
                match message.message {
                    WM_STOP => break,
                    WM_APP => {
                        adapter.apply(&update);
                        for action in adapter.drain_actions() {
                            let _ = action_sender.send(action);
                        }
                    }
                    _ => unsafe {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    },
                }
            }
            drop(adapter);
            let _ = unsafe { DestroyWindow(hwnd) };
        });
        let hwnd = ready_receiver
            .recv()
            .map_err(|_| ProbeError::Native("provider thread vanished".into()))?
            .map_err(|()| ProbeError::Native("provider window could not be created".into()))?;
        let client = unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
        }
        .map_err(native)?;
        Ok(Self {
            hwnd,
            join: Some(join),
            actions,
            client,
        })
    }

    fn live(&self) -> Result<Vec<(ProbeElement, IUIAutomationElement)>, ProbeError> {
        unsafe {
            let root = self
                .client
                .ElementFromHandle(HWND(self.hwnd as *mut _))
                .map_err(native)?;
            let condition = self.client.CreateTrueCondition().map_err(native)?;
            let all = root
                .FindAll(TreeScope_Descendants, &condition)
                .map_err(native)?;
            (0..all.Length().map_err(native)?)
                .map(|index| {
                    let element = all.GetElement(index).map_err(native)?;
                    let rect = element.CurrentBoundingRectangle().map_err(native)?;
                    Ok((
                        ProbeElement {
                            name: element.CurrentName().map_err(native)?.to_string(),
                            kind: ProbeKind::from_control_type(
                                element.CurrentControlType().map_err(native)?.0,
                            ),
                            bounds: (rect.left, rect.top, rect.right, rect.bottom),
                        },
                        element,
                    ))
                })
                .collect()
        }
    }

    pub fn elements(&self) -> Result<Vec<ProbeElement>, ProbeError> {
        Ok(self.live()?.into_iter().map(|(seen, _)| seen).collect())
    }

    pub fn wait_for(
        &self,
        timeout: Duration,
        mut ready: impl FnMut(&[ProbeElement]) -> bool,
    ) -> Result<Vec<ProbeElement>, ProbeError> {
        let deadline = Instant::now() + timeout;
        loop {
            let seen = self.elements()?;
            if ready(&seen) {
                return Ok(seen);
            }
            if Instant::now() >= deadline {
                return Err(ProbeError::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn element_named(&self, name: &str) -> Result<IUIAutomationElement, ProbeError> {
        self.live()?
            .into_iter()
            .find(|(seen, _)| seen.name == name)
            .map(|(_, element)| element)
            .ok_or_else(|| ProbeError::NotFound(name.to_owned()))
    }

    pub fn invoke(&self, name: &str) -> Result<(), ProbeError> {
        let element = self.element_named(name)?;
        let pattern: IUIAutomationInvokePattern =
            unsafe { element.GetCurrentPatternAs(UIA_InvokePatternId) }.map_err(native)?;
        unsafe { pattern.Invoke() }.map_err(native)
    }

    pub fn set_value(&self, name: &str, value: &str) -> Result<(), ProbeError> {
        let element = self.element_named(name)?;
        let pattern: IUIAutomationValuePattern =
            unsafe { element.GetCurrentPatternAs(UIA_ValuePatternId) }.map_err(native)?;
        unsafe { pattern.SetValue(&value.into()) }.map_err(native)
    }

    pub fn value_of(&self, name: &str) -> Result<String, ProbeError> {
        let element = self.element_named(name)?;
        let pattern: IUIAutomationValuePattern =
            unsafe { element.GetCurrentPatternAs(UIA_ValuePatternId) }.map_err(native)?;
        unsafe { pattern.CurrentValue() }
            .map(|value| value.to_string())
            .map_err(native)
    }

    pub fn actions(&self) -> Vec<A11yActionRequest> {
        self.actions.try_iter().collect()
    }
}

impl Drop for UiaProbe {
    fn drop(&mut self) {
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_STOP,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
