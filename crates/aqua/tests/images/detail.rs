use std::sync::Arc;
use std::time::Duration;

use aqua::images::{ImageAssetBudget, ImageHandle, ImageStatus};
use aqua::{px, ui, ImageAssets, Instant, ManagedUiApp, Style, UiApp};
use aqua_core::{DisplayItem, Point, Rect};
use aqua_text::{SharedFonts, SharedRuns};

use crate::support;

fn png(width: u32, height: u32) -> Arc<[u8]> {
    let mut bytes = Vec::new();
    image::RgbImage::from_pixel(width, height, image::Rgb([40, 90, 200]))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    Arc::from(bytes)
}

fn shown(assets: ImageAssets, images: &[(&ImageHandle, Rect)]) -> ManagedUiApp {
    let items: Vec<_> = images
        .iter()
        .map(|(handle, bounds)| DisplayItem::Image {
            bounds: *bounds,
            layer: aqua_core::scene::media_layer(handle.id()),
            fit: aqua_core::image::ImageFit::Contain,
            position: Point::new(0.5, 0.5),
            source: Rect::from_edges(0.0, 0.0, 1.0, 1.0),
            opacity: 1.0,
            rounded_clip: 0.0,
        })
        .collect();
    let mut ui = UiApp::new(SharedFonts::new(), SharedRuns::new());
    ui.mount(move |_| {
        ui::Canvas::new()
            .style(Style::new().width(px(1000.0)).height(px(1000.0)))
            .on_paint(move |_| items.clone())
            .build()
    })
    .unwrap();
    ui.set_viewport(1000.0, 1000.0, 1.0);
    support::frame(&mut ui, Instant::ZERO);
    ManagedUiApp::new(ui, assets)
}

fn uploads(app: &mut ManagedUiApp, count: usize) -> Vec<(u32, u32)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut frames = Vec::new();
    while frames.len() < count {
        assert!(
            std::time::Instant::now() < deadline,
            "image upload timed out"
        );
        frames.extend(
            app.synchronize_images(Instant::ZERO)
                .unwrap()
                .iter()
                .map(|frame| (frame.width, frame.height)),
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    frames
}

#[test]
fn a_still_is_decoded_for_the_size_it_is_shown_at_and_reports_its_source_size() {
    let assets = ImageAssets::new(ImageAssetBudget::editor());
    let handle = assets.decode_preview(png(6000, 1500)).unwrap();
    let source = ImageStatus::Ready {
        width: 6000,
        height: 1500,
    };
    let mut app = shown(
        assets,
        &[(&handle, Rect::from_edges(0.0, 0.0, 1000.0, 250.0))],
    );
    assert_eq!(uploads(&mut app, 1)[0], (2048, 512));
    assert_eq!(handle.status(), source);

    app.ui_mut().set_viewport(1000.0, 1000.0, 3.0);
    support::frame(app.ui_mut(), Instant::from_nanos(1));
    assert_eq!(uploads(&mut app, 1)[0], (4096, 1024));
    assert_eq!(handle.status(), source);
    assert_eq!(app.gpu_image_bytes(), 4096 * 1024 * 4);

    app.ui_mut().set_viewport(1000.0, 1000.0, 1.0);
    support::frame(app.ui_mut(), Instant::from_nanos(2));
    assert_eq!(uploads(&mut app, 1)[0], (2048, 512));
    assert_eq!(handle.status(), source);
}

#[test]
fn a_still_is_never_decoded_beyond_its_source() {
    let assets = ImageAssets::new(ImageAssetBudget::editor());
    let handle = assets.decode_preview(png(300, 200)).unwrap();
    let mut app = shown(
        assets,
        &[(&handle, Rect::from_edges(0.0, 0.0, 900.0, 600.0))],
    );
    assert_eq!(uploads(&mut app, 1)[0], (300, 200));
    app.ui_mut().set_viewport(1000.0, 1000.0, 4.0);
    support::frame(app.ui_mut(), Instant::from_nanos(1));
    for _ in 0..50 {
        assert!(app
            .synchronize_images(Instant::from_nanos(1))
            .unwrap()
            .is_empty());
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn visible_stills_beyond_the_gpu_budget_are_decoded_smaller_instead_of_failing() {
    let budget = ImageAssetBudget {
        gpu_bytes: 3 * 1024 * 1024,
        ..ImageAssetBudget::editor()
    };
    let assets = ImageAssets::new(budget);
    let first = assets.decode_preview(png(2048, 1024)).unwrap();
    let second = assets.decode_preview(png(2048, 1024)).unwrap();
    let mut app = shown(
        assets,
        &[
            (&first, Rect::from_edges(0.0, 0.0, 1000.0, 500.0)),
            (&second, Rect::from_edges(0.0, 500.0, 1000.0, 1000.0)),
        ],
    );
    let mut sizes = uploads(&mut app, 2);
    sizes.sort_unstable();
    assert_eq!(sizes, vec![(724, 362), (1024, 512)]);
    assert!(app.gpu_image_bytes() <= budget.gpu_bytes);
    for handle in [&first, &second] {
        assert_eq!(
            handle.status(),
            ImageStatus::Ready {
                width: 2048,
                height: 1024
            }
        );
    }
}
