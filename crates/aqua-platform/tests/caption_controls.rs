use aqua_platform::chrome::{hit_test_chrome, ht, CaptionControl, DragRects, HitInput, RectI};

#[test]
fn authored_controls_override_dwm_strip_and_track_client_origin() {
    let rect = |left, right| RectI {
        left,
        top: 0,
        right,
        bottom: 36,
    };
    let drag = DragRects {
        no_drag: vec![rect(0, 138)],
        caption_controls: vec![
            (CaptionControl::Minimize, rect(0, 46)),
            (CaptionControl::Maximize, rect(46, 92)),
            (CaptionControl::Close, rect(92, 138)),
        ],
        ..Default::default()
    };
    let mut input = HitInput {
        point: (0, 0),
        window: RectI {
            left: 100,
            top: 100,
            right: 900,
            bottom: 700,
        },
        client_origin: (108, 108),
        border: 8,
        buttons: Some(RectI {
            left: 108,
            top: 108,
            right: 246,
            bottom: 144,
        }),
        drag_band: 40,
        drag: &drag,
        maximized: true,
        resizable: true,
        minimizable: true,
    };
    for (x, expected) in [(110, ht::MINBUTTON), (160, ht::MAXBUTTON), (210, ht::CLOSE)] {
        input.point = (x, 120);
        assert_eq!(hit_test_chrome(&input), expected);
    }
    input.resizable = false;
    input.point = (160, 120);
    assert_eq!(hit_test_chrome(&input), ht::CAPTION);
    input.minimizable = false;
    input.point = (110, 120);
    assert_eq!(hit_test_chrome(&input), ht::CAPTION);
    input.point = (247, 120);
    assert_eq!(hit_test_chrome(&input), ht::CAPTION);
    input.maximized = false;
    input.resizable = true;
    input.point = (160, 101);
    assert_eq!(hit_test_chrome(&input), ht::TOP);
}
