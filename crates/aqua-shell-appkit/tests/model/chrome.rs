use aqua_shell_appkit::model::chrome::{centred_buttons, double_click, DoubleClick};

#[test]
fn traffic_lights_sit_centred_in_the_title_band() {
    for (band, button, window) in [
        (40.0, 14.0, 600.0),
        (52.0, 16.0, 300.0),
        (28.0, 14.0, 480.0),
    ] {
        let strip = centred_buttons(band, button, window);
        let top_from_window_top = window - (strip.container_y + strip.container_height);
        let button_top = top_from_window_top + (strip.container_height - button);
        let centre = button_top + button / 2.0;
        assert!(
            (centre - band / 2.0).abs() < 1e-9,
            "band {band}: the button centre sits {centre} below the top"
        );
    }
}

#[test]
fn double_clicking_the_caption_follows_the_system_setting() {
    assert_eq!(double_click(None, None), DoubleClick::Zoom);
    assert_eq!(double_click(Some("Maximize"), None), DoubleClick::Zoom);
    assert_eq!(double_click(Some("Fill"), None), DoubleClick::Zoom);
    assert_eq!(
        double_click(Some("Minimize"), Some(false)),
        DoubleClick::Minimize
    );
    assert_eq!(double_click(Some("None"), Some(true)), DoubleClick::Nothing);
    assert_eq!(double_click(None, Some(true)), DoubleClick::Minimize);
}
