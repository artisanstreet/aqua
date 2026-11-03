use aqua::{px, ui, ImageAssets, Input, Instant, ManagedUiApp, Style, UiApp};
use aqua_core::{DisplayItem, Point, Rect, Size};
use aqua_render::{media::DecodedImage, RenderError, Renderer};
use aqua_text::{SharedFonts, SharedRuns};

#[test]
fn viewport_visibility_changes_upload_and_release_assets_with_an_unchanged_scene() {
    let renderer = match pollster::block_on(Renderer::headless(128, 64)) {
        Ok(renderer) => renderer,
        Err(RenderError::AdapterNotFound(_))
            if std::env::var("AQUA_REQUIRE_GPU").as_deref() != Ok("1") =>
        {
            eprintln!("SKIP-NO-ADAPTER");
            return;
        }
        Err(error) => panic!("{error}"),
    };
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
    ui.step(Input::Viewport {
        size: Size::new(64.0, 64.0),
        scale: 1.0,
    })
    .unwrap();
    ui.step(Input::Frame(Instant::ZERO)).unwrap();
    let scene_revision = ui.scene().revision();
    let mut app = ManagedUiApp::new(ui, assets);
    assert!(app
        .synchronize_images(renderer.device(), renderer.queue(), Instant::ZERO)
        .unwrap()
        .is_empty());
    assert_eq!(app.gpu_image_bytes(), 0);
    app.ui_mut()
        .step(Input::Viewport {
            size: Size::new(128.0, 64.0),
            scale: 1.0,
        })
        .unwrap();
    app.ui_mut()
        .step(Input::Frame(Instant::from_nanos(1)))
        .unwrap();
    assert_eq!(app.ui().scene().revision(), scene_revision);
    let uploaded = app
        .synchronize_images(renderer.device(), renderer.queue(), Instant::from_nanos(1))
        .unwrap();
    assert_eq!(uploaded.len(), 1);
    assert_eq!(uploaded[0].layer, layer);
    assert!(app.gpu_image_bytes() > 0);
    app.ui_mut()
        .step(Input::Viewport {
            size: Size::new(64.0, 64.0),
            scale: 1.0,
        })
        .unwrap();
    app.ui_mut()
        .step(Input::Frame(Instant::from_nanos(2)))
        .unwrap();
    assert_eq!(app.ui().scene().revision(), scene_revision);
    assert!(app
        .synchronize_images(renderer.device(), renderer.queue(), Instant::from_nanos(2))
        .unwrap()
        .is_empty());
    assert_eq!(app.take_released_images(), vec![layer]);
    assert_eq!(app.gpu_image_bytes(), 0);
}
