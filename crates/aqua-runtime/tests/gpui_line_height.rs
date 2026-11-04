use aqua_core::{ColorProfile, DisplayItem};
use aqua_runtime::Runtime;
use aqua_text::{RunId, SharedFonts, SharedRuns, TextStyle};
use aqua_ui::element::ui::Text;
use aqua_ui::layout::{ProfiledCosmicMeasurer, TextMeasurer};

#[test]
fn profile_applies_to_measurement_and_retained_shaping() {
    for (profile, expected) in [
        (ColorProfile::AquaLinear, 27.5),
        (ColorProfile::GpuiWindowsEncoded, 27.0),
    ] {
        let fonts = SharedFonts::new();
        let runs = SharedRuns::new();
        let style = TextStyle::default();
        let measure = ProfiledCosmicMeasurer {
            fonts: fonts.clone(),
            scale: 1.25,
            gpui_line_height: profile == ColorProfile::GpuiWindowsEncoded,
            gpui_wrap: profile == ColorProfile::GpuiWindowsEncoded,
        };
        let measured = measure.measure_styled("first\nsecond", &style, &[], None, true);
        let mut runtime = Runtime::new(fonts, runs.clone()).with_color_profile(profile);
        runtime
            .mount(|_| {
                Text::new("first\nsecond")
                    .multiline(true)
                    .text_style(style)
                    .build()
            })
            .unwrap();
        runtime.set_viewport(300.0, 200.0, 1.25);
        runtime.build_frame().unwrap();
        let run = runtime
            .scene()
            .ordered()
            .find_map(|(_, item)| match item {
                DisplayItem::Text { run, .. } => Some(run.clone()),
                _ => None,
            })
            .unwrap();
        let registry = runs.lock();
        let retained = registry
            .resolve_expected(RunId(run.id), run.revision)
            .unwrap();
        let lines = &retained.layout.laid_out().unwrap().lines;
        assert_eq!(lines[1].y_top - lines[0].y_top, expected);
        assert_eq!(
            measured.h * 1.25,
            retained.layout.laid_out().unwrap().size.1
        );
    }
}
