use aqua_core::{
    vector::{FillRule, GpuiPath, GpuiPathStyle, PathCommand},
    DisplayItem, GpuiBackground, GpuiHsla, Point, Rect, Transform2D,
};

#[test]
fn gpui_path_places_local_geometry_before_element_and_device_scaling() {
    let mut path = GpuiPath {
        commands_local: vec![PathCommand::Move(Point::new(3.0, 4.0))],
        origin_window_logical: Point::new(20.0, 30.0),
        element_transform: Transform2D::IDENTITY,
        style: GpuiPathStyle::Fill {
            rule: FillRule::NonZero,
        },
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 0.5,
            a: 1.0,
        }),
    };
    path.apply_element_transform(
        Transform2D::scale(2.0, 2.0).then(Transform2D::translate(5.0, -3.0)),
    );
    assert_eq!(
        path.device_position(Point::new(3.0, 4.0), 1.5),
        Point::new(76.5, 97.5)
    );
    assert_eq!(
        path.commands_local,
        vec![PathCommand::Move(Point::new(3.0, 4.0))]
    );
    assert_eq!(path.origin_window_logical, Point::new(20.0, 30.0));

    path.apply_element_transform(Transform2D::translate(-4.0, 7.0));
    assert_eq!(
        path.window_logical_position(Point::new(3.0, 4.0)),
        Point::new(47.0, 72.0)
    );
    assert_eq!(
        path.device_position(Point::new(3.0, 4.0), 1.5),
        Point::new(70.5, 108.0)
    );

    let bounds = Rect::from_edges(40.0, 65.0, 52.0, 79.0);
    let item = DisplayItem::GpuiPath {
        transformed_window_logical_bounds: bounds,
        path: std::sync::Arc::new(path),
    };
    assert_eq!(item.bounds(), Some(bounds));
}
