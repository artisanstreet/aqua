#![cfg(windows)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use aqua::config::Titlebar;
use aqua::{px, Aqua, Context, IntoView};
use serde_json::Value;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_EditControlTypeId, UIA_InvokePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, PostMessageW, WM_CHAR, WM_CLOSE,
};

const TITLE: &str = "aqua host native test";

fn app(cx: &Context) -> impl IntoView {
    let (read, write) = cx.scope().signal(String::from("draft"));
    let bump = write.clone();
    Aqua::Box()
        .styles([Aqua::Style().column().width(px(400.0)).height(px(200.0))])
        .child(
            Aqua::Button()
                .styles([Aqua::Style().width(px(160.0)).height(px(40.0))])
                .on_activate(move || bump.update(|text| text.push('!')))
                .child(Aqua::Text("Press").build())
                .build(),
        )
        .child(
            Aqua::Text("")
                .bind((read, write))
                .editable(true)
                .styles([Aqua::Style().width(px(300.0)).height(px(32.0))])
                .build(),
        )
        .build()
}

fn ask(port: u16, command: &str) -> Option<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    writeln!(stream, "{command}").ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

fn find<'a>(node: &'a Value, role: &str) -> Option<&'a Value> {
    if node["role"] == role {
        return Some(node);
    }
    node["children"]
        .as_array()?
        .iter()
        .find_map(|child| find(child, role))
}

fn tree_value(port: u16, role: &str, field: &str) -> Option<Value> {
    let reply = ask(port, "tree")?;
    find(&reply["tree"], role).map(|node| node[field].clone())
}

fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> Result<T, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(value) = probe() {
            return Ok(value);
        }
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}"));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn controls(automation: &IUIAutomation, hwnd: HWND) -> Vec<(i32, IUIAutomationElement)> {
    unsafe {
        let root = automation.ElementFromHandle(hwnd).unwrap();
        let all = root
            .FindAll(
                TreeScope_Descendants,
                &automation.CreateTrueCondition().unwrap(),
            )
            .unwrap();
        (0..all.Length().unwrap())
            .map(|index| {
                let element = all.GetElement(index).unwrap();
                (element.CurrentControlType().unwrap().0, element)
            })
            .collect()
    }
}

fn client(port: u16) -> Result<(), String> {
    let trace = |step: &str| {
        let active = unsafe { GetForegroundWindow() };
        eprintln!(
            "host_native: client {step} (foreground is test window: {})",
            active == unsafe { FindWindowW(None, w!("aqua host native test")) }.unwrap_or_default()
        );
    };
    let hwnd = wait_for("the host window", || {
        let hwnd = unsafe { FindWindowW(None, w!("aqua host native test")) }.ok()?;
        (!hwnd.0.is_null()).then_some(hwnd)
    })?;
    trace("window found");
    let foreground_before = unsafe { GetForegroundWindow() };
    wait_for("the inspector tree", || {
        tree_value(port, "Button", "id").filter(|id| !id.is_null())
    })?;
    let draft = wait_for("the editable value", || {
        tree_value(port, "TextInput", "value").filter(|value| !value.is_null())
    })?;
    if draft != "draft" {
        return Err(format!("unexpected initial value {draft}"));
    }

    trace("tree ok");
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| e.to_string())?;
    let button = wait_for("a UIA button", || {
        controls(&automation, hwnd)
            .into_iter()
            .find(|(kind, element)| {
                *kind == UIA_ButtonControlTypeId.0
                    && unsafe { element.CurrentName() }.is_ok_and(|name| name == "Press")
            })
            .map(|(_, element)| element)
    })?;
    trace("uia button found");
    let rect = unsafe { button.CurrentBoundingRectangle() }.map_err(|e| e.to_string())?;
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return Err(format!("empty UIA bounds {rect:?}"));
    }
    let invoke: IUIAutomationInvokePattern =
        unsafe { button.GetCurrentPatternAs(UIA_InvokePatternId) }.map_err(|e| e.to_string())?;
    trace("invoking");
    let invoked = unsafe { invoke.Invoke() };
    trace("invoke returned");
    invoked.map_err(|e| e.to_string())?;
    wait_for("the Invoke action to reach the runtime", || {
        tree_value(port, "TextInput", "value").filter(|value| value == "draft!")
    })?;

    let edit = wait_for("a UIA edit", || {
        controls(&automation, hwnd)
            .into_iter()
            .find(|(kind, _)| *kind == UIA_EditControlTypeId.0)
            .map(|(_, element)| element)
    })?;
    trace("before UIA SetFocus");
    unsafe { edit.SetFocus() }.map_err(|e| e.to_string())?;
    wait_for("the UIA focus action to reach the runtime", || {
        tree_value(port, "TextInput", "focused").filter(|value| value == true)
    })?;
    unsafe { PostMessageW(Some(hwnd), WM_CHAR, WPARAM('x' as usize), LPARAM(1)) }
        .map_err(|e| e.to_string())?;
    wait_for("typed text through the message hook", || {
        tree_value(port, "TextInput", "value")
            .and_then(|value| value.as_str().map(str::to_owned))
            .filter(|value| value.contains('x'))
    })
    .map_err(|e| {
        format!(
            "{e}; last value {:?}",
            tree_value(port, "TextInput", "value")
        )
    })?;

    wait_for("the first presented frame", || {
        ask(port, "stats").filter(|stats| stats["frames_presented"].as_u64().unwrap_or(0) > 0)
    })?;
    let foreground_after = unsafe { GetForegroundWindow() };
    eprintln!(
        "host_native: foreground changed to the test window: {}",
        foreground_after == hwnd && foreground_before != hwnd
    );
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn main() {
    let port = free_port();
    std::env::set_var("AQUA_INSPECTOR", port.to_string());
    std::env::set_var("AQUA_SMOKE_MS", "60000");
    std::env::set_var("AQUA_RENDER_WHEN_COVERED", "1");
    let (done, outcome) = mpsc::channel();
    thread::spawn(move || {
        let outcome = client(port);
        if let Err(message) = &outcome {
            eprintln!("host_native: client failed: {message}");
            if let Ok(hwnd) = unsafe { FindWindowW(None, w!("aqua host native test")) } {
                let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
            }
        }
        let _ = done.send(outcome);
    });
    let result = Aqua::App()
        .config(
            Aqua::Config().window(
                Aqua::Window()
                    .title(TITLE)
                    .size(px(480.0), px(320.0))
                    .titlebar(Titlebar::Native),
            ),
        )
        .run(app);
    if let Err(error) = result {
        eprintln!("host_native: run failed: {error}");
        std::process::exit(1);
    }
    match outcome.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => eprintln!("host_native: ok"),
        Ok(Err(message)) => {
            eprintln!("host_native: FAILED: {message}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("host_native: FAILED: client did not finish before the host exited");
            std::process::exit(1);
        }
    }
}
