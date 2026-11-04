use aqua_a11y_uia::{inspect, render_text};
use aqua_core::{NodeId, NumericRangeSemantics, RawId, Rect, Role, SemanticNode};
use aqua_protocol::{A11yNode, AccessTreeUpdate};

fn id(index: u32) -> NodeId {
    NodeId::from_raw(RawId {
        index,
        generation: 1,
    })
}

fn node(semantic: SemanticNode, children: Vec<NodeId>) -> A11yNode {
    A11yNode {
        text: None,
        bounds: Some(Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        }),
        semantic,
        children,
    }
}

#[test]
fn inspector_shows_roles_state_and_actions_from_the_tree_uia_sees() {
    let mut toggle = SemanticNode::new(id(2), Role::Checkbox);
    toggle.label = "Dark mode".into();
    toggle.checked = Some(true);
    let mut slider = SemanticNode::new(id(3), Role::Slider);
    slider.label = "Volume".into();
    slider.numeric_range = Some(NumericRangeSemantics {
        min: 0.0,
        max: 10.0,
        value: 4.0,
        step: 1.0,
    });
    let mut field = SemanticNode::new(id(4), Role::TextField);
    field.value_text = Some("hello".into());
    field.focused = true;
    let update = AccessTreeUpdate {
        root: id(1),
        focus: id(4),
        nodes: vec![
            node(
                SemanticNode::new(id(1), Role::Group),
                vec![id(2), id(3), id(4)],
            ),
            node(toggle, vec![]),
            node(slider, vec![]),
            node(field, vec![]),
        ],
    };
    let root = inspect(&update).expect("tree");
    assert_eq!(root.children.len(), 3);
    assert_eq!(root.children[0].toggled.as_deref(), Some("true"));
    assert!(root.children[0].actions.contains(&"click"));
    assert_eq!(root.children[1].numeric, Some(4.0));
    assert!(root.children[1].actions.contains(&"increment"));
    assert_eq!(root.children[2].value.as_deref(), Some("hello"));
    assert!(root.children[2].focused);
    let text = render_text(&root);
    assert!(text.contains("CheckBox \"Dark mode\" toggled=true"));
    assert!(text.contains("TextInput value=\"hello\""));
}
