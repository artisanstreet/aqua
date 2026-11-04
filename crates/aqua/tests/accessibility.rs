#![cfg(windows)]

use std::{cell::Cell, rc::Rc, sync::Arc};

use aqua::{
    ui::{Box as UiBox, Button, Text},
    UiApp,
};
use aqua_a11y_uia::{build_tree_update, pack_node_id, A11yBridge};
use aqua_core::Role;
use aqua_protocol::{A11yAction, A11yActionData, A11yActionRequest, Machine, RuntimeInput};
use aqua_text::{SharedFonts, SharedRuns};

#[test]
fn native_value_and_invoke_requests_update_the_real_bound_field_and_button() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let (read, write) = app.scope().signal("draft".to_owned());
    let pressed = Rc::new(Cell::new(0));
    let callback = pressed.clone();
    app.mount(|_| {
        UiBox::new()
            .child(
                Text::builder()
                    .bind((read.clone(), write))
                    .placeholder("Message")
                    .build(),
            )
            .child(
                Button::new()
                    .on_activate(move || callback.set(callback.get() + 1))
                    .child(Text::builder().arg("Send").build())
                    .build(),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(400.0, 200.0, 1.0);
    app.build_frame().unwrap();
    let (root, focus, nodes) = app.a11y_nodes().unwrap();
    let field = nodes
        .iter()
        .find(|node| node.semantic.role == Role::TextField)
        .unwrap()
        .semantic
        .node;
    let button = nodes
        .iter()
        .find(|node| node.semantic.role == Role::Button)
        .unwrap()
        .semantic
        .node;
    let update = build_tree_update(root, focus, &nodes);
    let native_field = update
        .nodes
        .iter()
        .find(|(id, _)| *id == pack_node_id(field))
        .unwrap();
    assert!(native_field
        .1
        .supports_action(aqua_a11y_uia::accesskit::Action::SetValue));
    let bridge = A11yBridge::new(Arc::new(|| {}));
    bridge.native_action(&aqua_a11y_uia::accesskit::ActionRequest {
        action: aqua_a11y_uia::accesskit::Action::SetValue,
        target_tree: aqua_a11y_uia::accesskit::TreeId::ROOT,
        target_node: native_field.0,
        data: Some(aqua_a11y_uia::accesskit::ActionData::Value(
            "updated\r\nmessage".into(),
        )),
    });
    let requests = bridge.drain_actions();
    assert_eq!(requests.len(), 1);
    for request in requests {
        app.step(RuntimeInput::Access(request));
    }
    app.sync().unwrap();
    assert_eq!(read.get(), "updated message");
    app.step(RuntimeInput::Access(A11yActionRequest {
        node: button,
        action: A11yAction::Press,
        data: A11yActionData::None,
    }));
    assert_eq!(pressed.get(), 1);
    let mut semantics = app.tree().get(button).unwrap().semantics.clone();
    semantics.disabled = true;
    app.tree_mut().set_semantics(button, semantics);
    app.step(RuntimeInput::Access(A11yActionRequest {
        node: button,
        action: A11yAction::Press,
        data: A11yActionData::None,
    }));
    assert_eq!(pressed.get(), 1);
}

#[test]
fn password_content_is_protected_in_accessibility_and_clipboard() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        Text::builder()
            .arg("secret phrase")
            .editable(true)
            .password(true)
            .placeholder("Password")
            .build()
    })
    .unwrap();
    app.set_viewport(400.0, 200.0, 1.0);
    app.build_frame().unwrap();
    let (root, focus, nodes) = app.a11y_nodes().unwrap();
    let field = nodes
        .iter()
        .find(|node| node.semantic.role == Role::PasswordField)
        .unwrap();
    assert!(!format!("{nodes:?}").contains("secret phrase"));
    let update = build_tree_update(root, focus, &nodes);
    assert_eq!(
        update
            .nodes
            .iter()
            .find(|(id, _)| *id == pack_node_id(field.semantic.node))
            .unwrap()
            .1
            .role(),
        aqua_a11y_uia::accesskit::Role::PasswordInput
    );
    app.tree_mut().set_focused(Some(field.semantic.node));
    app.tree_mut()
        .set_text_selection(field.semantic.node, Some(aqua_text::Selection::new(0, 13)));
    assert_eq!(app.selected_text(), None);
}
