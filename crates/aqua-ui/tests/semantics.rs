use aqua_core::Role;
use aqua_ui::{
    style::LayoutStyle,
    tree::{NodeKind, NodeSemantics, NodeSpec, TextBacking, TextContent, UiTree},
};

#[test]
fn placeholder_names_the_field_without_entering_its_editable_value() {
    let mut tree = UiTree::new();
    let mut text = TextContent::editable("", false);
    text.placeholder = "Search".to_owned();
    let node = tree.mount_spec(NodeSpec::new(NodeKind::text(text), LayoutStyle::default()));
    tree.set_semantics(
        node,
        NodeSemantics {
            role: Role::TextField,
            ..NodeSemantics::default()
        },
    );
    let semantics = || {
        tree.emit_semantics()
            .into_iter()
            .find(|value| value.node == node)
            .unwrap()
    };
    assert_eq!(semantics().value_text.as_deref(), Some(""));
    tree.edit_text(node, |text| text.insert_at_caret("query"));
    let typed = tree
        .emit_semantics()
        .into_iter()
        .find(|value| value.node == node)
        .unwrap();
    assert_eq!(typed.value_text.as_deref(), Some("query"));
    assert_eq!(typed.label, "Search");
    tree.edit_text(node, |text| {
        if let TextBacking::Editable(editor) = &mut text.backing {
            editor.undo();
        }
    });
    let undone = tree
        .emit_semantics()
        .into_iter()
        .find(|value| value.node == node)
        .unwrap();
    assert_eq!(undone.value_text.as_deref(), Some(""));
    assert_eq!(undone.label, "Search");
}

mod scoped {
    use aqua_core::{LiveRegion, NumericRangeSemantics, Role, SemanticNode};
    use aqua_reactive::Runtime;
    use aqua_ui::binding::View;
    use aqua_ui::element::ui::Text;
    use aqua_ui::element::{Box, Element};
    use aqua_ui::events::{focus_next, focus_prev};
    use aqua_ui::tree::UiTree;

    fn button(label: &'static str) -> Element {
        Box::new()
            .build()
            .with_key(label)
            .role(Role::Button)
            .aria_label(label)
            .focusable(true)
    }

    fn named<'a>(nodes: &'a [SemanticNode], label: &str) -> Option<&'a SemanticNode> {
        nodes.iter().find(|node| node.label == label)
    }

    fn render(view: &mut View, tree: &mut UiTree, content: Element) {
        view.set_content(tree, vec![content]).unwrap();
        tree.drain_commands();
    }

    fn page(search: bool) -> Element {
        let mut root = Box::new()
            .child(button("Workspace"))
            .child(button("Sidebar"));
        if search {
            root = root.child(
                Box::new()
                    .child(button("Query"))
                    .child(button("Result"))
                    .build()
                    .with_key("search")
                    .role(Role::Dialog)
                    .aria_label("Search")
                    .modal_scope(),
            );
        }
        root.build()
    }

    #[test]
    fn a_modal_scope_hides_the_background_traps_tab_and_restores_focus() {
        let runtime = Runtime::new();
        let mut tree = UiTree::new();
        let mut view = View::new(runtime.create_scope(), tree.command_sink());
        render(&mut view, &mut tree, page(false));
        let workspace = named(&tree.emit_semantics(), "Workspace").unwrap().node;
        assert!(tree.set_focused(Some(workspace)));

        render(&mut view, &mut tree, page(true));
        let nodes = tree.emit_semantics();
        assert!(named(&nodes, "Workspace").is_none());
        assert!(named(&nodes, "Sidebar").is_none());
        assert!(named(&nodes, "Search").unwrap().modal);
        let query = named(&nodes, "Query").unwrap().node;
        let result = named(&nodes, "Result").unwrap().node;
        assert!(nodes.iter().all(|node| !node.children.contains(&workspace)));

        assert!(tree.set_focused(Some(query)));
        assert_eq!(focus_next(&mut tree), Some(result));
        assert_eq!(focus_next(&mut tree), Some(query));
        assert_eq!(focus_prev(&mut tree), Some(result));

        render(&mut view, &mut tree, page(false));
        assert_eq!(tree.focused(), Some(workspace));
        assert!(named(&tree.emit_semantics(), "Sidebar").is_some());
    }

    #[test]
    fn tab_from_focus_left_behind_a_modal_enters_the_modal() {
        let runtime = Runtime::new();
        let mut tree = UiTree::new();
        let mut view = View::new(runtime.create_scope(), tree.command_sink());
        render(&mut view, &mut tree, page(false));
        let workspace = named(&tree.emit_semantics(), "Workspace").unwrap().node;

        for (step, expected) in [
            (focus_next as fn(&mut UiTree) -> _, "Query"),
            (focus_prev, "Result"),
        ] {
            render(&mut view, &mut tree, page(false));
            assert!(tree.set_focused(Some(workspace)));
            render(&mut view, &mut tree, page(true));
            let target = named(&tree.emit_semantics(), expected).unwrap().node;
            assert_eq!(step(&mut tree), Some(target));
        }
    }

    #[test]
    fn content_metadata_reaches_the_semantic_tree() {
        let runtime = Runtime::new();
        let mut tree = UiTree::new();
        let mut view = View::new(runtime.create_scope(), tree.command_sink());
        let content = || {
            Box::new()
                .child(
                    Text::builder()
                        .arg("Streaming answer")
                        .build()
                        .aria_live(LiveRegion::Polite)
                        .aria_busy(true)
                        .aria_language("nb"),
                )
                .child(
                    Box::new()
                        .child(Text::builder().arg("reel digits").build())
                        .build()
                        .aria_hidden(),
                )
                .child(
                    Box::new()
                        .build()
                        .role(Role::Slider)
                        .aria_label("Threshold")
                        .aria_range(NumericRangeSemantics {
                            min: 0.0,
                            max: 10.0,
                            value: 4.0,
                            step: 1.0,
                        }),
                )
                .child(
                    Box::new()
                        .build()
                        .role(Role::Group)
                        .aria_label("Chart series")
                        .focusable(true),
                )
                .child(button("Model"))
                .child(
                    Box::new()
                        .build()
                        .role(Role::Option)
                        .aria_label("Second")
                        .aria_active_descendant(),
                )
                .build()
        };
        render(&mut view, &mut tree, content());
        let nodes = tree.emit_semantics();
        let answer = named(&nodes, "Streaming answer").unwrap();
        assert_eq!(answer.live, Some(LiveRegion::Polite));
        assert!(answer.busy);
        assert_eq!(answer.language.as_deref(), Some("nb"));
        assert!(named(&nodes, "reel digits").is_none());
        assert_eq!(
            named(&nodes, "Threshold").unwrap().numeric_range,
            Some(NumericRangeSemantics {
                min: 0.0,
                max: 10.0,
                value: 4.0,
                step: 1.0,
            })
        );
        assert!(named(&nodes, "Chart series").unwrap().focusable);
        let model = named(&nodes, "Model").unwrap().node;
        let second = named(&nodes, "Second").unwrap().node;
        assert_eq!(named(&nodes, "Model").unwrap().active_descendant, None);
        assert!(tree.set_focused(Some(model)));
        assert_eq!(
            named(&tree.emit_semantics(), "Model")
                .unwrap()
                .active_descendant,
            Some(second)
        );
    }
}
