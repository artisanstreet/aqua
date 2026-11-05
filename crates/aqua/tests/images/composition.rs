use aqua::images::DecodedImage;
use aqua::{ImageAssets, Instant, ManagedUiApp, Style, UiApp, px, ui};
use aqua_core::{Rect, Size};
use aqua_text::{SharedFonts, SharedRuns};

use crate::support;

#[test]
fn public_managed_media_uploads_once_crops_renders_and_releases_removed_layers() {
    let (fonts, runs) = (SharedFonts::new(), SharedRuns::new());
    let painters = support::painters(&fonts, &runs);
    #[cfg(windows)]
    assert!(painters.len() > 1, "no D3D12 adapter available");
    for painter in painters {
        public_managed_media_uploads_once_crops_renders_and_releases_removed_layers_on(
            painter,
            fonts.clone(),
            runs.clone(),
        );
    }
}

fn public_managed_media_uploads_once_crops_renders_and_releases_removed_layers_on(
    mut renderer: support::Painter,
    fonts: SharedFonts,
    runs: SharedRuns,
) {
    let assets = ImageAssets::default();
    let handle = assets
        .insert(DecodedImage::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap())
        .unwrap();
    let mut ui = UiApp::new(fonts.clone(), runs.clone());
    ui.mount(|_| {
        ui::Media::new(handle.clone())
            .fit(aqua_core::image::ImageFit::Fill)
            .source_rect(Rect {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            })
            .style(Style::new().width(px(64.0)).height(px(64.0)))
            .build()
    })
    .unwrap();
    ui.set_viewport(64.0, 64.0, 1.0);
    support::frame(&mut ui, Instant::ZERO);
    let mut app = ManagedUiApp::new(ui, assets.clone());
    let uploaded = app.synchronize_images(Instant::ZERO).unwrap();
    assert_eq!(uploaded.len(), 1);
    for image in uploaded {
        renderer.register_image(image.layer, image.width, image.height, &image.rgba);
    }
    let pixels = renderer.paint(app.ui_mut().scene_mut(), Size::new(64.0, 64.0), 1.0);
    let middle = &pixels.pixel(32, 32);
    assert!(
        middle[1] > 240 && middle[0] < 16,
        "managed source crop: {middle:?}"
    );
    assert!(app.synchronize_images(Instant::ZERO).unwrap().is_empty());
    let root = app.ui().tree().root().unwrap();
    app.ui_mut().tree_mut().remove(root);
    support::frame(app.ui_mut(), Instant::from_nanos(1));
    app.synchronize_images(Instant::from_nanos(1)).unwrap();
    assert_eq!(
        app.take_released_images(),
        vec![aqua_core::scene::media_layer(handle.id())]
    );
    assert_eq!(app.gpu_image_bytes(), 0);
    drop(handle);
    drop(app);
    assert_eq!(assets.stats().decoded_bytes, 0);
}

#[test]
fn managed_gif_selects_frames_by_host_time_and_gpu_admission_reports_failure() {
    let (fonts, runs) = (SharedFonts::new(), SharedRuns::new());
    let painters = support::painters(&fonts, &runs);
    #[cfg(windows)]
    assert!(painters.len() > 1, "no D3D12 adapter available");
    for painter in painters {
        managed_gif_selects_frames_by_host_time_and_gpu_admission_reports_failure_on(
            painter,
            fonts.clone(),
            runs.clone(),
        );
    }
}

fn managed_gif_selects_frames_by_host_time_and_gpu_admission_reports_failure_on(
    mut renderer: support::Painter,
    fonts: SharedFonts,
    runs: SharedRuns,
) {
    use aqua::{Application, ImageAssetBudget, ImageAssetError, ImageStatus};
    use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
    use aqua_protocol::window::ObservedWindowState;
    use std::sync::Arc;
    use std::time::{Duration, Instant as WallInstant};
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .unwrap();
        for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            encoder
                .encode_frame(image::Frame::from_parts(
                    image::RgbaImage::from_pixel(2, 2, image::Rgba(color)),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(30, 1),
                ))
                .unwrap();
        }
    }
    let assets = ImageAssets::default();
    let handle = assets.decode(Arc::<[u8]>::from(bytes)).unwrap();
    let timeout = WallInstant::now() + Duration::from_secs(5);
    while handle.status() == ImageStatus::Pending {
        assert!(WallInstant::now() < timeout);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        handle.status(),
        ImageStatus::Ready {
            width: 2,
            height: 2
        }
    );
    let mut ui = UiApp::new(fonts.clone(), runs.clone());
    ui.mount(|_| {
        ui::Media::new(handle.clone())
            .style(Style::new().width(px(32.0)).height(px(32.0)).opacity(0.0))
            .build()
    })
    .unwrap();
    ui.set_viewport(32.0, 32.0, 1.0);
    let mut app = ManagedUiApp::new(ui, assets);
    let window = ObservedWindowState::new(
        AquaWindowId::new(1).unwrap(),
        "animated-media".into(),
        LogicalSize::new(32.0, 32.0),
    );
    assert_eq!(app.build_scene(&window, Instant::ZERO), None);
    assert!(app.synchronize_images(Instant::ZERO).unwrap().is_empty());
    assert_eq!(app.gpu_image_bytes(), 0);
    let root = app.ui().tree().root().unwrap();
    let mut root_paint = app.ui().tree().paint(root).unwrap();
    root_paint.opacity = Some(1.0);
    app.ui_mut().tree_mut().set_paint(root, root_paint);
    for (time, expected) in [
        (0, [255, 0, 0, 255]),
        (30_000_000, [0, 255, 0, 255]),
        (60_000_000, [255, 0, 0, 255]),
    ] {
        let now = Instant::from_nanos(time);
        assert_eq!(
            app.build_scene(&window, now),
            Some(Instant::from_nanos(time + 30_000_000))
        );
        let uploaded = app.synchronize_images(now).unwrap();
        assert_eq!(uploaded.len(), 1);
        for image in uploaded {
            renderer.register_image(image.layer, image.width, image.height, &image.rgba);
        }
        let pixels = renderer.paint(app.ui_mut().scene_mut(), Size::new(32.0, 32.0), 1.0);
        assert_eq!(pixels.pixel(16, 16), expected);
        assert!(app.synchronize_images(now).unwrap().is_empty());
    }
    let assets = ImageAssets::new(ImageAssetBudget {
        gpu_bytes: 1,
        ..Default::default()
    });
    let handle = assets
        .insert(DecodedImage::from_rgba(1, 1, vec![255; 4]).unwrap())
        .unwrap();
    let mut ui = UiApp::new(fonts.clone(), runs.clone());
    ui.mount(|_| {
        ui::Media::new(handle.clone())
            .style(Style::new().width(px(32.0)).height(px(32.0)))
            .build()
    })
    .unwrap();
    ui.set_viewport(32.0, 32.0, 1.0);
    let mut app = ManagedUiApp::new(ui, assets);
    app.build_scene(&window, Instant::ZERO);
    assert!(matches!(
        app.synchronize_images(Instant::ZERO),
        Err(ImageAssetError::OverBudget)
    ));
    assert!(matches!(
        handle.status(),
        ImageStatus::Failed(ImageAssetError::Upload(_))
    ));
    assert_eq!(app.gpu_image_bytes(), 0);
}

#[test]
fn cancelling_mounted_image_invalidates_retained_pixels_without_scene_changes() {
    let (fonts, runs) = (SharedFonts::new(), SharedRuns::new());
    let painters = support::painters(&fonts, &runs);
    #[cfg(windows)]
    assert!(painters.len() > 1, "no D3D12 adapter available");
    for painter in painters {
        cancelling_mounted_image_invalidates_retained_pixels_without_scene_changes_on(
            painter,
            fonts.clone(),
            runs.clone(),
        );
    }
}

fn cancelling_mounted_image_invalidates_retained_pixels_without_scene_changes_on(
    mut renderer: support::Painter,
    fonts: SharedFonts,
    runs: SharedRuns,
) {
    let assets = ImageAssets::default();
    let handle = assets
        .insert(DecodedImage::from_rgba(1, 1, vec![255, 0, 0, 255]).unwrap())
        .unwrap();
    let mut ui = UiApp::new(fonts.clone(), runs.clone());
    ui.mount(|_| {
        ui::Media::new(handle.clone())
            .style(Style::new().width(px(32.0)).height(px(32.0)))
            .build()
    })
    .unwrap();
    ui.set_viewport(32.0, 32.0, 1.0);
    support::frame(&mut ui, Instant::ZERO);
    let mut app = ManagedUiApp::new(ui, assets);
    for image in app.synchronize_images(Instant::ZERO).unwrap() {
        renderer.register_image(image.layer, image.width, image.height, &image.rgba);
    }
    let revision = app.ui().scene().revision();
    let pixels = renderer.paint(app.ui_mut().scene_mut(), Size::new(32.0, 32.0), 1.0);
    assert_eq!(pixels.pixel(16, 16), [255, 0, 0, 255]);
    handle.cancel();
    app.synchronize_images(Instant::ZERO).unwrap();
    for layer in app.take_released_images() {
        renderer.release_image(layer);
    }
    assert_eq!(app.ui().scene().revision(), revision);
    let pixels = renderer.paint(app.ui_mut().scene_mut(), Size::new(32.0, 32.0), 1.0);
    assert_eq!(pixels.pixel(16, 16), [0, 0, 0, 0]);
}
