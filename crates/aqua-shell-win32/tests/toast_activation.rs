#[cfg(windows)]
mod scripted {
    use aqua_shell_win32::notifications::activation::{
        activation_url, handle_launch, unregister_protocol, Launch,
    };
    use aqua_shell_win32::notifications::{AppIdentity, NotificationCenter, NotificationResponse};
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use windows::core::{w, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, SW_HIDE,
    };

    fn pump() {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            let _ = unsafe { TranslateMessage(&message) };
            unsafe { DispatchMessageW(&message) };
        }
    }

    fn click(url: &str) {
        let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        let result =
            unsafe { ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), None, None, SW_HIDE) };
        assert!(
            result.0 as isize > 32,
            "protocol launch failed: {:?}",
            result.0
        );
    }

    fn next(receiver: &Receiver<NotificationResponse>) -> NotificationResponse {
        let started = Instant::now();
        loop {
            pump();
            if let Ok(response) = receiver.try_recv() {
                return response;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "no activation arrived"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn run() {
        let identity =
            AppIdentity::new("Aqua.ToastActivationTest", "Aqua toast activation").unwrap();
        let (sender, receiver) = channel();
        let sender = Arc::new(Mutex::new(sender));
        let center = NotificationCenter::new(
            identity.clone(),
            Arc::new(move |response| {
                let _ = sender.lock().unwrap().send(response);
            }),
        )
        .expect("center");

        click(&activation_url(&identity, "host/thread 1", Some("open")));
        let action = next(&receiver);
        assert_eq!(action.tag, "host/thread 1");
        assert_eq!(action.action_id.as_deref(), Some("open"));
        println!("test protocol_action_reaches_the_running_instance ... ok");

        click(&activation_url(&identity, "host/thread 2", None));
        let body = next(&receiver);
        assert_eq!(body.tag, "host/thread 2");
        assert_eq!(body.action_id, None);
        println!("test protocol_body_click_reaches_the_running_instance ... ok");

        drop(center);
        let _ = unregister_protocol();
    }

    pub fn forwarder() -> bool {
        matches!(handle_launch(), Launch::Handled)
    }
}

fn main() {
    #[cfg(windows)]
    {
        if scripted::forwarder() {
            return;
        }
        scripted::run();
    }
}
