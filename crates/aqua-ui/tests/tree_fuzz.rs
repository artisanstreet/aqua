use aqua_core::Size;
use aqua_style::Paint;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::motion::{Easing, Motion, MotionSpec, Presence};
use aqua_ui::style::{Axis, LayoutStyle, Length};
use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, UiTree};
use std::rc::Rc;

const FADE: Motion = Motion::tween(0.2, Easing::Linear);

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

fn spec(key: u32, exit: bool, depth: u32, rng: &mut Lcg) -> NodeSpec {
    let paint = Paint {
        motion: exit.then(|| {
            Rc::new(MotionSpec {
                presence: Presence::new().exit(FADE),
                ..MotionSpec::default()
            })
        }),
        ..Paint::default()
    };
    let children = match depth {
        0 => Vec::new(),
        _ => (0..rng.next(3))
            .map(|i| spec(key * 10 + i as u32 + 1, rng.next(2) == 0, depth - 1, rng))
            .collect(),
    };
    let mut node = NodeSpec::new(
        NodeKind::Box(BoxContent {
            paint: paint.clone(),
        }),
        LayoutStyle {
            axis: Axis::Column,
            width: Length::Px(60.0),
            height: Length::Px(20.0),
            ..LayoutStyle::default()
        },
    )
    .keyed(key);
    node.paint = paint;
    node.children = children;
    node
}

fn layout(tree: &mut UiTree) {
    tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
        .unwrap();
}

fn run(seed: u64) {
    let mut rng = Lcg(seed);
    let mut tree = UiTree::new();
    let root = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle {
            axis: Axis::Column,
            width: Length::Px(400.0),
            height: Length::Px(400.0),
            ..LayoutStyle::default()
        },
    ));
    layout(&mut tree);
    let mut popups: Vec<aqua_core::NodeId> = Vec::new();
    for _ in 0..120 {
        match rng.next(6) {
            0 | 1 => {
                let keys: Vec<u32> = (0..8).filter(|_| rng.next(2) == 0).collect();
                let specs = keys
                    .into_iter()
                    .map(|key| spec(key + 1, rng.next(2) == 0, 2, &mut rng))
                    .collect();
                tree.sync_children(root, specs);
            }
            2 => {
                let popup = tree
                    .insert_spec(root, 0, spec(100 + rng.next(50) as u32, false, 2, &mut rng))
                    .unwrap();
                if tree.mark_portal(popup) {
                    popups.push(popup);
                }
            }
            3 => {
                if let Some(popup) = popups.pop() {
                    tree.remove(popup);
                }
            }
            4 => {
                for node in tree.exiting_nodes() {
                    if rng.next(2) == 0 {
                        tree.finish_exit(node);
                    }
                }
            }
            _ => {
                let kids = tree.get(root).unwrap().children.clone();
                if !kids.is_empty() {
                    let target = kids[rng.next(kids.len() as u64) as usize];
                    tree.move_child(root, target, rng.next(kids.len() as u64) as usize);
                }
            }
        }
        layout(&mut tree);
    }
}

#[test]
fn random_structural_edits_never_leave_stale_layout_nodes() {
    for seed in 0..100 {
        run(seed);
    }
}
