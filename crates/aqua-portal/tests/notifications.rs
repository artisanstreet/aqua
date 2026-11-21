#![cfg(target_os = "linux")]

mod support;

use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use aqua_portal::notifications::{
    native_tag, AppIdentity, Notification, NotificationAction, NotificationCenter,
    NotificationError, NotificationResponse, Route,
};
use aqua_portal::testing::PrivateBus;
use support::{desktop, freedesktop_notifications, next, Desktop, Event, PORTAL_PATH, WAIT};

fn identity() -> AppIdentity {
    AppIdentity::new("com.artisanstreet.Test", "Aqua Test").unwrap()
}

fn center(bus: &PrivateBus, first: Route) -> (NotificationCenter, Receiver<NotificationResponse>) {
    let (sender, responses) = channel();
    let sender = std::sync::Mutex::new(sender);
    let center = NotificationCenter::on(
        bus.bus(),
        identity(),
        Arc::new(move |response| {
            let _ = sender.lock().unwrap().send(response);
        }),
        first,
    )
    .unwrap();
    (center, responses)
}

fn message(tag: &str, body: &str) -> Notification {
    Notification {
        tag: tag.into(),
        title: "Build finished".into(),
        body: body.into(),
        actions: vec![NotificationAction {
            id: "open-log".into(),
            label: "Open log".into(),
        }],
    }
}

#[test]
fn a_notification_action_invocation_reaches_the_respond_callback() {
    let Some(bus) = support::bus() else { return };
    let (portal, events) = desktop(
        &bus,
        Desktop {
            notifications: true,
            ..Desktop::default()
        },
    );
    let (mut center, responses) = center(&bus, Route::Portal);
    center.show(message("build-1", "All green")).unwrap();
    let Event::Added {
        id,
        title,
        body,
        default_action,
        buttons,
    } = next(&events)
    else {
        panic!("the portal receives AddNotification");
    };
    assert_eq!(id, native_tag("build-1"));
    assert_eq!(
        (title.as_str(), body.as_str()),
        ("Build finished", "All green")
    );
    assert_eq!(buttons.len(), 1);
    assert_eq!(buttons[0].0, "Open log");
    portal.emit(
        PORTAL_PATH,
        "org.freedesktop.portal.Notification",
        "ActionInvoked",
        &(
            id.as_str(),
            buttons[0].1.as_str(),
            Vec::<zbus::zvariant::OwnedValue>::new(),
        ),
    );
    assert_eq!(
        responses.recv_timeout(WAIT).unwrap(),
        NotificationResponse {
            tag: "build-1".into(),
            action_id: Some("open-log".into()),
        }
    );

    center.show(message("build-2", "Clicked")).unwrap();
    let Event::Added { id, .. } = next(&events) else {
        panic!("the portal receives the second notification");
    };
    portal.emit(
        PORTAL_PATH,
        "org.freedesktop.portal.Notification",
        "ActionInvoked",
        &(
            id.as_str(),
            default_action.unwrap().as_str(),
            Vec::<zbus::zvariant::OwnedValue>::new(),
        ),
    );
    assert_eq!(
        responses.recv_timeout(WAIT).unwrap(),
        NotificationResponse {
            tag: "build-2".into(),
            action_id: None,
        }
    );
    assert_eq!(center.route(), Some(Route::Portal));
}

#[test]
fn withdraw_removes_the_notification() {
    let Some(bus) = support::bus() else { return };
    let (_portal, portal_events) = desktop(
        &bus,
        Desktop {
            notifications: true,
            ..Desktop::default()
        },
    );
    let (mut portal_center, _) = center(&bus, Route::Portal);
    portal_center.show(message("sync", "Syncing")).unwrap();
    next(&portal_events);
    portal_center.withdraw("sync").unwrap();
    assert_eq!(
        next(&portal_events),
        Event::Removed {
            id: native_tag("sync"),
        }
    );

    let (_server, events) = freedesktop_notifications(&bus, false);
    let (mut fdo_center, _) = center(&bus, Route::Freedesktop);
    fdo_center.show(message("sync", "Syncing")).unwrap();
    let Event::Notified { id, .. } = next(&events) else {
        panic!("the notification server receives Notify");
    };
    fdo_center.withdraw("sync").unwrap();
    assert_eq!(next(&events), Event::Closed { id });
}

#[test]
fn showing_a_tag_again_replaces_the_shown_notification() {
    let Some(bus) = support::bus() else { return };
    let (_server, events) = freedesktop_notifications(&bus, false);
    let (mut center, _) = center(&bus, Route::Freedesktop);
    center.show(message("download", "10%")).unwrap();
    let Event::Notified { id, replaces, .. } = next(&events) else {
        panic!("the notification server receives Notify");
    };
    assert_eq!(replaces, 0);
    center.show(message("download", "90%")).unwrap();
    let Event::Notified { replaces, body, .. } = next(&events) else {
        panic!("the notification server receives the replacement");
    };
    assert_eq!((replaces, body.as_str()), (id, "90%"));
}

#[test]
fn without_a_notification_portal_the_freedesktop_service_is_used() {
    let Some(bus) = support::bus() else { return };
    let (server, events) = freedesktop_notifications(&bus, false);
    let (mut center, responses) = center(&bus, Route::Portal);
    center.show(message("mail", "One new message")).unwrap();
    let Event::Notified {
        id,
        summary,
        actions,
        desktop_entry,
        ..
    } = next(&events)
    else {
        panic!("the notification server receives Notify");
    };
    assert_eq!(center.route(), Some(Route::Freedesktop));
    assert_eq!(summary, "Build finished");
    assert_eq!(desktop_entry.as_deref(), Some("com.artisanstreet.Test"));
    assert_eq!(actions.len(), 4);
    assert_eq!(actions[3], "Open log");
    server.emit(
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "ActionInvoked",
        &(id, actions[0].as_str()),
    );
    assert_eq!(
        responses.recv_timeout(WAIT).unwrap(),
        NotificationResponse {
            tag: "mail".into(),
            action_id: None,
        }
    );
}

#[test]
fn body_markup_is_escaped_for_servers_that_parse_it() {
    let Some(bus) = support::bus() else { return };
    let (_server, events) = freedesktop_notifications(&bus, true);
    let (mut center, _) = center(&bus, Route::Freedesktop);
    center.show(message("chat", "<b>Tom & Jerry</b>")).unwrap();
    let Event::Notified { body, .. } = next(&events) else {
        panic!("the notification server receives Notify");
    };
    assert_eq!(body, "&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;");
}

#[test]
fn without_any_notification_service_show_reports_unsupported() {
    let Some(bus) = support::bus() else { return };
    let (mut center, _) = center(&bus, Route::Portal);
    assert_eq!(
        center.show(message("lost", "Nobody listens")),
        Err(NotificationError::Unsupported)
    );
    assert_eq!(center.route(), None);
}
