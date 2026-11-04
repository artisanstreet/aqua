use aqua_shell_win32::notifications::{
    AppIdentity, Notification, NotificationAction, NotificationError,
};

fn notification() -> Notification {
    Notification {
        tag: "host/thread".into(),
        title: "Waiting <&> \"ready\"".into(),
        body: "Emoji 👩🏽‍💻\nSecond line & a </text>".into(),
        actions: vec![NotificationAction {
            id: "open&<\"thread".into(),
            label: "Open <thread>".into(),
        }],
    }
}

#[test]
fn notification_content_and_actions_round_trip_without_xml_injection() {
    let notification = notification();
    let xml = notification.xml().unwrap();
    let document = roxmltree::Document::parse(&xml).unwrap();
    let texts: Vec<_> = document
        .descendants()
        .filter(|node| node.has_tag_name("text"))
        .map(|node| node.text().unwrap())
        .collect();
    assert_eq!(texts, [&notification.title, &notification.body]);
    let action = document
        .descendants()
        .find(|node| node.has_tag_name("action"))
        .unwrap();
    assert_eq!(
        action.attribute("arguments"),
        Some(notification.actions[0].id.as_str())
    );
    assert_eq!(
        action.attribute("content"),
        Some(notification.actions[0].label.as_str())
    );
    assert_eq!(
        document
            .descendants()
            .filter(|node| node.has_tag_name("text"))
            .count(),
        2
    );
}

#[test]
fn malformed_identity_and_duplicate_action_ids_are_rejected() {
    assert!(AppIdentity::new("com.artisanstreet.editor", "Artisan Editor").is_ok());
    for identifier in ["", "has/slash", "has space", "nul\0id"] {
        assert_eq!(
            AppIdentity::new(identifier, "Editor"),
            Err(NotificationError::InvalidIdentity)
        );
    }
    let mut value = notification();
    value.actions.push(value.actions[0].clone());
    assert_eq!(value.xml(), Err(NotificationError::InvalidContent));
    value.actions.pop();
    value.body.push('\0');
    assert_eq!(value.xml(), Err(NotificationError::InvalidContent));
}

#[test]
fn arbitrary_thread_tags_are_bounded_and_replacements_keep_the_same_native_tag() {
    let mut first = notification();
    first.tag = "unicode 👩🏽‍💻/".repeat(100);
    let mut replacement = first.clone();
    replacement.body = "updated".into();
    assert!(first.native_tag().len() <= 64);
    assert_eq!(first.native_tag(), replacement.native_tag());
    replacement.tag.push('2');
    assert_ne!(first.native_tag(), replacement.native_tag());
}
