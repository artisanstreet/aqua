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

#[test]
fn protocol_activation_urls_round_trip_hostile_tags_and_actions() {
    use aqua_shell_win32::notifications::activation::{
        activation_url, parse_activation_url, ActivationUrlError,
    };
    let identity = AppIdentity::new("com.artisanstreet.editor", "Editor").unwrap();
    let tag = "host/thread \"1\" 👩🏽‍💻 %41 & <x>";
    let url = activation_url(&identity, tag, Some("open/\u{0}?#"));
    assert!(url.is_ascii() && !url.contains(' ') && !url.contains('"'));
    let parsed = parse_activation_url(&url).unwrap();
    assert_eq!(parsed.identifier, "com.artisanstreet.editor");
    assert_eq!(parsed.response.tag, tag);
    assert_eq!(parsed.response.action_id.as_deref(), Some("open/\u{0}?#"));
    let body = parse_activation_url(&activation_url(&identity, tag, None)).unwrap();
    assert_eq!(body.response.action_id, None);
    assert_eq!(
        parse_activation_url("https://app/a/b"),
        Err(ActivationUrlError::WrongScheme)
    );
    for bad in [
        "aqua-toast://app/",
        "aqua-toast://app/only",
        "aqua-toast://other/a/b",
        "aqua-toast://app/a//c",
        "aqua-toast://app/a/b/c/d",
        "aqua-toast://app/a/%FF",
    ] {
        assert_eq!(parse_activation_url(bad), Err(ActivationUrlError::Malformed), "{bad}");
    }
}

#[test]
fn protocol_toasts_carry_activation_urls_for_the_body_and_every_action() {
    let identity = AppIdentity::new("com.artisanstreet.editor", "Editor").unwrap();
    let notification = notification();
    let xml = notification.xml_for(Some(&identity)).unwrap();
    let document = roxmltree::Document::parse(&xml).unwrap();
    let toast = document.root_element();
    assert_eq!(toast.attribute("activationType"), Some("protocol"));
    let launch = aqua_shell_win32::notifications::activation::parse_activation_url(
        toast.attribute("launch").unwrap(),
    )
    .unwrap();
    assert_eq!(launch.response.tag, notification.tag);
    assert_eq!(launch.response.action_id, None);
    let action = document
        .descendants()
        .find(|node| node.has_tag_name("action"))
        .unwrap();
    assert_eq!(action.attribute("activationType"), Some("protocol"));
    let parsed = aqua_shell_win32::notifications::activation::parse_activation_url(
        action.attribute("arguments").unwrap(),
    )
    .unwrap();
    assert_eq!(parsed.response.action_id.as_deref(), Some(notification.actions[0].id.as_str()));
}
