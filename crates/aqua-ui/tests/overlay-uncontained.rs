use aqua_core::{Point, Rect, Size};
use aqua_ui::overlay::{place_overlay_uncontained, Align, Placement, Side};

#[test]
fn anchored_menu_keeps_its_requested_side_and_alignment_outside_viewport() {
    let placement = Placement {
        preferred: Side::Below,
        align: Align::Start,
        gap: 6.0,
        margin: 8.0,
        flip: true,
        centered: false,
    };
    let anchor = Rect::from_origin_size(Point::new(270.0, 185.0), Size::new(90.0, 20.0));
    let placed = place_overlay_uncontained(anchor, Size::new(160.0, 120.0), &placement);
    assert_eq!(placed.side, Side::Below);
    assert!(!placed.flipped);
    assert_eq!(
        placed.bounds,
        Rect::from_origin_size(Point::new(270.0, 211.0), Size::new(160.0, 120.0))
    );
}

#[test]
fn measured_popup_accepts_content_sized_root_and_keeps_legacy_fixed_requirement() {
    use aqua_ui::{
        layout::NaiveMeasurer,
        overlay::{DismissPolicy, Modality, OverlayKind},
        overlay_host::{HostError, OverlayHost, PopupRequest},
        style::{LayoutStyle, Length},
        tree::{BoxContent, NodeKind, NodeSpec, UiTree},
    };
    let mut tree = UiTree::new();
    let owner = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle {
            width: Length::Px(300.0),
            height: Length::Px(200.0),
            ..LayoutStyle::default()
        },
    ));
    tree.compute_layout(Size::new(300.0, 200.0), &NaiveMeasurer)
        .unwrap();
    let request = || PopupRequest {
        owner,
        parent: None,
        kind: OverlayKind::Popover,
        modality: Modality::NonModal,
        dismiss: DismissPolicy::LIGHT,
        content: NodeSpec::new(NodeKind::Box(BoxContent::default()), LayoutStyle::default()).child(
            NodeSpec::new(
                NodeKind::Box(BoxContent::default()),
                LayoutStyle {
                    width: Length::Px(90.0),
                    height: Length::Px(40.0),
                    ..LayoutStyle::default()
                },
            ),
        ),
        placement: Placement::below(),
        autofocus: None,
        focus_root: false,
    };
    let viewport = Rect::from_origin_size(Point::ZERO, Size::new(300.0, 200.0));
    let mut host = OverlayHost::new(viewport);
    assert_eq!(
        host.open_popup(&mut tree, request()),
        Err(HostError::ContentSizeNotFixed)
    );
    let popup = host
        .open_popup_measured(&mut tree, request(), None, &NaiveMeasurer)
        .unwrap();
    let bounds = host.service().bounds(popup).unwrap();
    assert_eq!(bounds.size(), Size::new(90.0, 40.0));
}
