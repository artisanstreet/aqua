use aqua::images::DecodedImage;
use aqua::{px, ui, Application, ImageAssets, Instant, ManagedUiApp, Style, UiApp};
use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
use aqua_protocol::window::ObservedWindowState;
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};

fn observed(id: u64) -> ObservedWindowState {
    ObservedWindowState {
        id: AquaWindowId::new(id).unwrap(),
        title: format!("window-{id}"),
        inner_size: LogicalSize::new(64.0, 64.0),
        scale_factor: 1.0,
        focused: false,
        visible: true,
        occluded: false,
        maximized: false,
    }
}

fn media(
    handle: aqua::ImageHandle,
) -> impl FnOnce(&aqua_reactive::Scope) -> aqua_ui::element::Element {
    move |_| {
        ui::Media::new(handle)
            .style(Style::new().width(px(32.0)).height(px(32.0)))
            .build()
    }
}

#[test]
fn every_window_uploads_and_releases_only_its_own_visible_images() {
    let (fonts, runs) = (SharedFonts::new(), SharedRuns::new());
    let assets = ImageAssets::default();
    let first_image = assets
        .insert(DecodedImage::from_rgba(1, 1, vec![255, 0, 0, 255]).unwrap())
        .unwrap();
    let second_image = assets
        .insert(DecodedImage::from_rgba(1, 1, vec![0, 255, 0, 255]).unwrap())
        .unwrap();
    let first_layer = aqua_core::scene::media_layer(first_image.id());
    let second_layer = aqua_core::scene::media_layer(second_image.id());
    let factory_fonts = fonts.clone();
    let factory_runs = runs.clone();
    let factory_image = second_image.clone();
    let ui = UiApp::new(fonts, runs).with_window_factory(move |_| {
        let mut runtime = Runtime::new(factory_fonts.clone(), factory_runs.clone());
        runtime.mount(media(factory_image.clone()))?;
        Ok(runtime)
    });
    let mut ui = ui;
    ui.mount(media(first_image.clone())).unwrap();
    ui.set_viewport(64.0, 64.0, 1.0);
    let mut app = ManagedUiApp::new(ui, assets);
    let (one, two) = (observed(1), observed(2));
    let mut aqua = aqua::Aqua::new(aqua::AquaConfig::default()).unwrap();
    aqua.add_headless_window(one.clone());
    aqua.add_headless_window(two.clone());
    aqua.dispatch_start(&mut app, one.id);
    aqua.dispatch_start(&mut app, two.id);
    app.ui_mut()
        .runtime_for_mut(two.id)
        .unwrap()
        .set_viewport(64.0, 64.0, 1.0);

    app.build_scene(&one, Instant::ZERO);
    app.build_scene(&two, Instant::ZERO);
    let first = app.synchronize_images_for(one.id, Instant::ZERO).unwrap();
    let second = app.synchronize_images_for(two.id, Instant::ZERO).unwrap();
    assert_eq!(
        first.iter().map(|frame| frame.layer).collect::<Vec<_>>(),
        vec![first_layer]
    );
    assert_eq!(
        second.iter().map(|frame| frame.layer).collect::<Vec<_>>(),
        vec![second_layer]
    );
    assert_eq!(app.gpu_image_bytes(), 8);

    aqua.dispatch_window_event(
        &mut app,
        two.id,
        &aqua_protocol::WindowEvent::Destroyed(two.id),
    );
    assert_eq!(app.gpu_image_bytes(), 4);
    assert!(app
        .synchronize_images_for(one.id, Instant::from_nanos(1))
        .unwrap()
        .is_empty());
}
