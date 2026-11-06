#![cfg(windows)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant as WallInstant};

use aqua::{px, ui, ImageAssets, ImageHandle, ImageStatus, Instant, ManagedUiApp, Style, UiApp};
use aqua_core::Size;
use aqua_text::{SharedFonts, SharedRuns};

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="32"><style>.t { font-family: Arial; font-size: 2em; fill: #ff0000 }</style><text class="t" x="4" y="24">Hi</text></svg>"##;

fn wait(handle: &ImageHandle) -> ImageStatus {
    let deadline = WallInstant::now() + Duration::from_secs(20);
    loop {
        let status = handle.status();
        if status != ImageStatus::Pending {
            return status;
        }
        assert!(WallInstant::now() < deadline, "svg decode never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Shown {
    uploaded_ink: usize,
    painted_ink: usize,
}

fn show(assets: ImageAssets, fonts: &SharedFonts, runs: &SharedRuns) -> Vec<Shown> {
    let handle = assets.decode(Arc::<[u8]>::from(SVG.as_bytes())).unwrap();
    assert!(matches!(wait(&handle), ImageStatus::Ready { .. }));
    support::painters(fonts, runs)
        .into_iter()
        .map(|mut renderer| {
            let mut ui = UiApp::new(fonts.clone(), runs.clone());
            let shown = handle.clone();
            ui.mount(move |_| {
                ui::Media::new(shown.clone())
                    .fit(aqua_core::image::ImageFit::Fill)
                    .style(Style::new().width(px(96.0)).height(px(32.0)))
                    .build()
            })
            .unwrap();
            ui.set_viewport(96.0, 32.0, 1.0);
            support::frame(&mut ui, Instant::ZERO);
            let mut app = ManagedUiApp::new(ui, assets.clone());
            let uploaded = app.synchronize_images(Instant::ZERO).unwrap();
            assert_eq!(uploaded.len(), 1);
            let mut uploaded_ink = 0;
            for image in uploaded {
                uploaded_ink = image
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[3] > 32)
                    .count();
                renderer.register_image(image.layer, image.width, image.height, &image.rgba);
            }
            let frame = renderer.paint(app.ui_mut().scene_mut(), Size::new(96.0, 32.0), 1.0);
            let painted_ink = frame
                .pixels()
                .filter(|p| p[3] > 32 && p[0] > 160 && p[1] < 96 && p[2] < 96)
                .count();
            Shown {
                uploaded_ink,
                painted_ink,
            }
        })
        .collect()
}

#[test]
fn svg_text_is_outlined_on_the_decode_worker_and_reaches_every_renderer() {
    let (fonts, runs) = (SharedFonts::new(), SharedRuns::new());
    let assets = ImageAssets::default();
    assets.set_fonts(fonts.clone());
    let shown = show(assets, &fonts, &runs);
    assert!(shown.len() > 1, "no D3D12 adapter available");
    for each in &shown {
        assert!(
            each.uploaded_ink > 120,
            "uploaded ink {}",
            each.uploaded_ink
        );
        assert!(
            each.painted_ink > 80,
            "painted red ink {}",
            each.painted_ink
        );
    }
}

#[test]
fn decoding_without_fonts_renders_no_text() {
    let (fonts, runs) = (SharedFonts::new(), SharedRuns::new());
    let silent = show(ImageAssets::default(), &fonts, &runs);
    assert!(!silent.is_empty());
    assert!(silent
        .iter()
        .all(|each| each.uploaded_ink == 0 && each.painted_ink == 0));
}
