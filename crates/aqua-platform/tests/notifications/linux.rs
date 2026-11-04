use super::*;

#[test]
fn caller_action_ids_never_collide_with_transport_ids() {
    let ids = HashMap::from([
        (transport_action(0), DEFAULT_ACTION.to_owned()),
        (transport_action(1), CLOSED_ACTION.to_owned()),
    ]);
    assert_eq!(response_action(DEFAULT_ACTION, &ids), Some(None));
    assert_eq!(response_action(CLOSED_ACTION, &ids), None);
    assert_eq!(
        response_action(&transport_action(0), &ids),
        Some(Some(DEFAULT_ACTION.to_owned()))
    );
    assert_eq!(
        response_action(&transport_action(1), &ids),
        Some(Some(CLOSED_ACTION.to_owned()))
    );
    assert_eq!(response_action("unknown", &ids), None);
}

#[test]
fn only_the_newest_generation_of_a_tag_may_respond() {
    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
    lock(&registry).insert(
        "thread".into(),
        Active {
            generation: 2,
            server_id: None,
        },
    );
    assert!(!remove_current(&registry, "thread", 1));
    assert!(remove_current(&registry, "thread", 2));
    assert!(!remove_current(&registry, "thread", 2));
}
