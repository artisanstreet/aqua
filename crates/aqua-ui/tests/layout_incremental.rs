use aqua_core::{NodeId, Size};
use aqua_style::{Axis, LayoutStyle, Length};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, ScrollContent, TextContent, UiTree};
use std::collections::HashMap;

const VIEWPORT: Size = Size { w: 400.0, h: 300.0 };

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
}

fn column(width: f32) -> LayoutStyle {
    LayoutStyle {
        axis: Axis::Column,
        width: Length::Px(width),
        ..LayoutStyle::default()
    }
}

fn row_spec(rng: &mut Lcg, depth: u32) -> NodeSpec {
    let style = LayoutStyle {
        axis: if rng.next(2) == 0 {
            Axis::Row
        } else {
            Axis::Column
        },
        padding: aqua_core::Insets::uniform(rng.next(4) as f32),
        gap: rng.next(3) as f32,
        ..LayoutStyle::default()
    };
    let mut node = NodeSpec::new(NodeKind::Box(BoxContent::default()), style);
    node.children = match depth {
        0 => vec![NodeSpec::new(
            NodeKind::text(TextContent::label(
                "word ".repeat(rng.next(12) as usize + 1),
            )),
            LayoutStyle::default(),
        )],
        _ => (0..rng.next(4) + 1)
            .map(|_| row_spec(rng, depth - 1))
            .collect(),
    };
    node
}

fn all_nodes(tree: &UiTree) -> Vec<NodeId> {
    tree.traverse()
}

fn snapshot(tree: &UiTree) -> HashMap<NodeId, (Option<aqua_core::Rect>, Option<Size>)> {
    all_nodes(tree)
        .into_iter()
        .map(|id| (id, (tree.rect(id), tree.content_size(id))))
        .collect()
}

#[test]
fn incremental_commit_matches_a_full_commit_after_random_mutations() {
    let mut rng = Lcg(7);
    let mut tree = UiTree::new();
    let mut scroll = NodeSpec::new(
        NodeKind::Scroll(ScrollContent::default()),
        LayoutStyle {
            height: Length::Px(250.0),
            ..column(380.0)
        },
    );
    scroll.children = (0..12).map(|_| row_spec(&mut rng, 3)).collect();
    let mut root = NodeSpec::new(NodeKind::Box(BoxContent::default()), column(400.0));
    root.children = vec![scroll, row_spec(&mut rng, 2)];
    let root = tree.mount_spec(root);
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();

    for step in 0..300 {
        let nodes = all_nodes(&tree);
        let pick = nodes[rng.next(nodes.len() as u64) as usize];
        match rng.next(5) {
            0 => {
                let mut style = tree.get(pick).unwrap().style.clone();
                style.padding = aqua_core::Insets::uniform(rng.next(6) as f32);
                tree.set_style(pick, style);
            }
            1 => {
                let mut style = tree.get(pick).unwrap().style.clone();
                style.width = Length::Px(20.0 + rng.next(300) as f32);
                tree.set_style(pick, style);
            }
            2 if pick != root => {
                tree.remove(pick);
            }
            3 => {
                let spec = row_spec(&mut rng, 2);
                if matches!(tree.get(pick).unwrap().kind, NodeKind::Box(_)) {
                    tree.insert_spec(pick, 0, spec);
                }
            }
            _ => {
                if matches!(tree.get(pick).unwrap().kind, NodeKind::Text(_)) {
                    tree.set_text(pick, "x ".repeat(rng.next(30) as usize + 1));
                }
            }
        }
        tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
        let incremental = snapshot(&tree);
        tree.invalidate_layout_cache();
        tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
        let full = snapshot(&tree);
        assert_eq!(incremental, full, "diverged at step {step}");
    }
}

#[test]
fn a_local_text_change_commits_only_its_neighbourhood() {
    let mut rng = Lcg(3);
    let mut tree = UiTree::new();
    let mut root = NodeSpec::new(NodeKind::Box(BoxContent::default()), column(400.0));
    root.children = (0..200)
        .map(|_| {
            let mut row = row_spec(&mut rng, 2);
            row.style.height = Length::Px(60.0);
            row.style.width = Length::Px(400.0);
            row
        })
        .collect();
    tree.mount_spec(root);
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    let full = tree.last_stats();
    assert!(full.nodes_traversed > 500);
    let leaf = all_nodes(&tree)
        .into_iter()
        .find(|id| matches!(tree.get(*id).unwrap().kind, NodeKind::Text(_)))
        .unwrap();
    tree.set_text(leaf, "short");
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    let stats = tree.last_stats();
    assert!(!stats.layout_skipped);
    assert!(stats.nodes_traversed < 20, "{stats:?}");
    assert!(stats.nodes_visited < 2 * 200 + 30, "{stats:?}");
}
