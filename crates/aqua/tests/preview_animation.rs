use std::sync::Arc;
use std::time::{Duration, Instant as WallInstant};

use aqua::{px, ui, Application, ImageAssets, ImageStatus, Instant, ManagedUiApp, Style, UiApp};
use aqua_platform::{geometry::LogicalSize, AquaWindowId, ObservedWindowState};
use aqua_render::{RenderError, Renderer};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::motion::ReducedMotion;
use aqua_ui::tree::{MediaContent, NodeKind};

fn gif() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Finite(1))
            .unwrap();
        for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            encoder
                .encode_frame(image::Frame::from_parts(
                    image::RgbaImage::from_pixel(1, 1, image::Rgba(color)),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(30, 1),
                ))
                .unwrap();
        }
    }
    bytes
}

fn paint(
    app: &mut ManagedUiApp,
    renderer: &mut Renderer,
    window: &ObservedWindowState,
    ms: u64,
) -> Vec<u8> {
    let now = Instant::from_nanos(ms * 1_000_000);
    app.build_scene(window, now);
    let images = app
        .synchronize_images(renderer.device(), renderer.queue(), now)
        .unwrap();
    for layer in app.take_released_images() {
        renderer.unregister_external(layer);
    }
    for image in images {
        renderer.register_external(image.layer, image.view, image.width, image.height);
    }
    let plan = renderer.prepare(app.ui_mut().scene_mut());
    renderer.draw(&plan).unwrap();
    renderer.readback().unwrap()
}

fn color(pixels: &[u8], x: usize) -> &[u8] {
    &pixels[(16 * 64 + x) * 4..][..4]
}

#[test]
fn previews_have_per_mount_clocks_and_pause_and_restart_without_losing_frame() {
    let mut renderer = match pollster::block_on(Renderer::headless(64, 32)) {
        Ok(renderer) => renderer,
        Err(RenderError::AdapterNotFound(_))
            if std::env::var("AQUA_REQUIRE_GPU").as_deref() != Ok("1") =>
        {
            return
        }
        Err(error) => panic!("{error}"),
    };
    let assets = ImageAssets::default();
    let image = assets.decode_preview(Arc::<[u8]>::from(gif())).unwrap();
    let timeout = WallInstant::now() + Duration::from_secs(5);
    while image.status() == ImageStatus::Pending {
        assert!(WallInstant::now() < timeout);
        std::thread::yield_now();
    }
    assert!(matches!(image.status(), ImageStatus::Ready { .. }));
    let mut ui = UiApp::new(SharedFonts::new(), SharedRuns::new());
    ui.mount(|_| {
        ui::Box::new()
            .style(Style::new().row().width(px(64.0)).height(px(32.0)))
            .child(
                ui::Media::new(image.clone())
                    .style(Style::new().width(px(32.0)).height(px(32.0)))
                    .build(),
            )
            .build()
    })
    .unwrap();
    let mut app = ManagedUiApp::new(ui, assets);
    let mut window = ObservedWindowState::new(
        AquaWindowId::new(1).unwrap(),
        "preview".into(),
        LogicalSize::new(64.0, 32.0),
    );
    window.focused = true;
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    assert_eq!(color(&paint(&mut app, &mut renderer, &window, 0), 16), red);
    assert_eq!(
        color(&paint(&mut app, &mut renderer, &window, 30), 16),
        green
    );
    let root = app.ui().tree().root().unwrap();
    let first = app.ui().tree().get(root).unwrap().children[0];
    let style = app.ui().tree().get(first).unwrap().style.clone();
    let insert = |app: &mut ManagedUiApp| {
        app.ui_mut().tree_mut().insert(
            Some(root),
            NodeKind::Media(MediaContent {
                image: Some(image.clone().into()),
                ..Default::default()
            }),
            style.clone(),
        )
    };
    let second = insert(&mut app);
    let pixels = paint(&mut app, &mut renderer, &window, 35);
    assert_eq!(color(&pixels, 16), green);
    assert_eq!(color(&pixels, 48), red);
    let pixels = paint(&mut app, &mut renderer, &window, 65);
    assert_eq!(color(&pixels, 16), red);
    assert_eq!(color(&pixels, 48), green);
    window.focused = false;
    paint(&mut app, &mut renderer, &window, 70);
    let pixels = paint(&mut app, &mut renderer, &window, 200);
    assert_eq!(color(&pixels, 16), red);
    assert_eq!(color(&pixels, 48), green);
    window.focused = true;
    paint(&mut app, &mut renderer, &window, 220);
    let pixels = paint(&mut app, &mut renderer, &window, 250);
    assert_eq!(color(&pixels, 16), green);
    assert_eq!(color(&pixels, 48), red);
    app.ui_mut()
        .host_mut()
        .motion_mut()
        .set_reduced_motion(ReducedMotion::Reduce, 0.255);
    paint(&mut app, &mut renderer, &window, 255);
    let pixels = paint(&mut app, &mut renderer, &window, 400);
    assert_eq!(color(&pixels, 16), green);
    assert_eq!(color(&pixels, 48), red);
    app.ui_mut()
        .host_mut()
        .motion_mut()
        .set_reduced_motion(ReducedMotion::Full, 0.410);
    paint(&mut app, &mut renderer, &window, 410);
    let pixels = paint(&mut app, &mut renderer, &window, 440);
    assert_eq!(color(&pixels, 16), red);
    assert_eq!(color(&pixels, 48), green);
    app.ui_mut().tree_mut().remove(second);
    paint(&mut app, &mut renderer, &window, 445);
    insert(&mut app);
    let pixels = paint(&mut app, &mut renderer, &window, 450);
    assert_eq!(color(&pixels, 16), red);
    assert_eq!(color(&pixels, 48), red);
}
