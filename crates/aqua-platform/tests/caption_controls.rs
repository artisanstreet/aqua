use aqua_platform::chrome::{CaptionControl, DragRects, HitInput, RectI, hit_test_chrome, ht};

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

#[test]
fn client_chrome_hit_prefers_controls_then_excludes_no_drag_from_drag() {
    use aqua_platform::chrome::{CaptionRegion, ClientChromeHit, DragRegions, client_chrome_hit};
    let regions = DragRegions {
        draggable: vec![(0.0, 0.0, 400.0, 36.0)],
        no_drag: vec![(100.0, 0.0, 50.0, 36.0)],
        caption_controls: vec![CaptionRegion {
            control: CaptionControl::Close,
            bounds: (354.0, 0.0, 46.0, 36.0),
        }],
    };
    assert_eq!(
        client_chrome_hit(&regions, 10.0, 10.0),
        ClientChromeHit::Drag
    );
    assert_eq!(
        client_chrome_hit(&regions, 120.0, 10.0),
        ClientChromeHit::Client
    );
    assert_eq!(
        client_chrome_hit(&regions, 360.0, 10.0),
        ClientChromeHit::Caption(CaptionControl::Close)
    );
    assert_eq!(
        client_chrome_hit(&regions, 10.0, 50.0),
        ClientChromeHit::Client
    );
}
