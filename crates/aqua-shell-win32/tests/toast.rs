#![cfg(windows)]

use aqua_shell_win32::notifications::{
    AppIdentity, Notification, NotificationAction, NotificationCenter, NotificationError,
};
use std::sync::Arc;

fn center() -> NotificationCenter {
    let identity = AppIdentity::new("Aqua.ShellTest", "Aqua shell test").unwrap();
    NotificationCenter::new(identity, Arc::new(|_| {})).expect("center")
}

fn toast(tag: &str) -> Notification {
    Notification {
        tag: tag.into(),
        title: "Aqua shell test".into(),
        body: "Automated toast check".into(),
        actions: vec![NotificationAction {
            id: "open".into(),
            label: "Open".into(),
        }],
    }
}

#[test]
fn toast_show_replace_and_withdraw_succeed_on_windows() {
    let mut center = center();
    center.show(toast("probe/one")).expect("show");
    center.show(toast("probe/one")).expect("replace");
    center.withdraw("probe/one").expect("withdraw");
    center
        .withdraw("probe/never-shown")
        .expect("withdraw unknown");
}

#[test]
fn invalid_content_never_reaches_the_native_toast_api() {
    let mut center = center();
    let mut bad = toast("probe/bad");
    bad.title = "bad\u{0}title".into();
    assert_eq!(center.show(bad), Err(NotificationError::InvalidContent));
}
