use aqua::images::DecodedImage;
use aqua::{px, ui, ImageAssets, Instant, ManagedUiApp, Style, UiApp};
use aqua_core::{DisplayItem, Point, Rect};
use aqua_text::{SharedFonts, SharedRuns};

use crate::support;

#[test]
fn viewport_visibility_changes_upload_and_release_assets_with_an_unchanged_scene() {
    let assets = ImageAssets::default();
    let handle = assets
        .insert(DecodedImage::from_rgba(1, 1, vec![255; 4]).unwrap())
        .unwrap();
    let layer = aqua_core::scene::media_layer(handle.id());
    let mut ui = UiApp::new(SharedFonts::new(), SharedRuns::new());
    ui.mount(|_| {
        ui::Canvas::new()
            .style(Style::new().width(px(128.0)).height(px(64.0)))
            .on_paint(move |_| {
                vec![DisplayItem::Image {
                    bounds: Rect::from_edges(80.0, 0.0, 112.0, 32.0),
                    layer,
                    fit: aqua_core::image::ImageFit::Fill,
                    position: Point::new(0.5, 0.5),
                    source: Rect::from_edges(0.0, 0.0, 1.0, 1.0),
                    opacity: 1.0,
                    rounded_clip: 0.0,
                }]
            })
            .build()
    })
    .unwrap();
    ui.set_viewport(64.0, 64.0, 1.0);
    support::frame(&mut ui, Instant::ZERO);
    let scene_revision = ui.scene().revision();
    let mut app = ManagedUiApp::new(ui, assets);
    assert!(app.synchronize_images(Instant::ZERO).unwrap().is_empty());
    assert_eq!(app.gpu_image_bytes(), 0);
    app.ui_mut().set_viewport(128.0, 64.0, 1.0);
    support::frame(app.ui_mut(), Instant::from_nanos(1));
    assert_eq!(app.ui().scene().revision(), scene_revision);
    let uploaded = app.synchronize_images(Instant::from_nanos(1)).unwrap();
    assert_eq!(uploaded.len(), 1);
    assert_eq!(uploaded[0].layer, layer);
    assert!(app.gpu_image_bytes() > 0);
    app.ui_mut().set_viewport(64.0, 64.0, 1.0);
    support::frame(app.ui_mut(), Instant::from_nanos(2));
    assert_eq!(app.ui().scene().revision(), scene_revision);
    assert!(app
        .synchronize_images(Instant::from_nanos(2))
        .unwrap()
        .is_empty());
    assert_eq!(app.take_released_images(), vec![layer]);
    assert_eq!(app.gpu_image_bytes(), 0);
}
