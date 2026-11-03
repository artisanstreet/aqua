use aqua_core::{geometry::CornerRadii, scene::ShadowLayer};
use aqua_render::{DisplayItem, Oklch, Rect, RenderError, Renderer, Scene};

fn new_renderer() -> Option<Renderer> {
    match pollster::block_on(Renderer::headless(80, 64)) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::AdapterNotFound(error)) if std::env::var("AQUA_REQUIRE_GPU").is_err() => {
            eprintln!("SKIP: {error}");
            None
        }
        Err(error) => panic!("{error}"),
    }
}

fn shadow(bounds: Rect, layer: ShadowLayer) -> DisplayItem {
    DisplayItem::Shadow {
        bounds,
        corners: CornerRadii::uniform(0.0),
        layer,
    }
}

fn alpha(pixels: &[u8], x: usize, y: usize) -> u8 {
    pixels[(y * 80 + x) * 4 + 3]
}

fn gaussian_alpha(x: f32, y: f32, sigma: f32) -> f32 {
    let erf = |value: f32| {
        let a = value.abs();
        let q = 1.0 + (0.278393 + (0.230389 + 0.078108 * a * a) * a) * a;
        value.signum() * (1.0 - 1.0 / (q * q * q * q))
    };
    let start = (-3.0 * sigma).clamp(y - 10.0, y + 10.0);
    let end = (3.0 * sigma).clamp(y - 10.0, y + 10.0);
    let step = (end - start) / 4.0;
    (0..4)
        .map(|i| {
            let sample = start + (i as f32 + 0.5) * step;
            let integral = 0.5
                * (erf((x + 10.0) * (0.5_f32.sqrt() / sigma))
                    - erf((x - 10.0) * (0.5_f32.sqrt() / sigma)));
            integral * (-(sample * sample) / (2.0 * sigma * sigma)).exp()
                / ((2.0 * std::f32::consts::PI).sqrt() * sigma)
                * step
        })
        .sum()
}

#[test]
fn shadow_draws_zero_blur_rims_and_gaussian_tail_and_repaints_old_bounds() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let bounds = Rect::from_edges(20.0, 20.0, 40.0, 40.0);
    let mut layer = ShadowLayer {
        dx: 0.0,
        dy: 0.0,
        blur: 0.0,
        spread: 2.0,
        inset: false,
        color: Oklch::BLACK,
    };
    let mut scene = Scene::new();
    let id = scene.insert(shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert!(alpha(&pixels, 18, 30) > 240);
    assert_eq!(alpha(&pixels, 25, 30), 0);
    layer.blur = 8.0;
    layer.spread = 0.0;
    layer.dx = 20.0;
    scene.update(id, shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    assert!(plan.partial);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(alpha(&pixels, 18, 30), 0);
    let expected = (gaussian_alpha(12.5, 0.5, 4.0) * 255.0).round() as i32;
    assert!((alpha(&pixels, 62, 30) as i32 - expected).abs() <= 3);
    assert_eq!(alpha(&pixels, 30, 30), 0);
    let mut fresh = new_renderer();
    if let Some(ref mut fresh) = fresh {
        let mut fresh_scene = Scene::new();
        fresh_scene.insert(shadow(bounds, layer));
        let plan = fresh.prepare(&mut fresh_scene);
        fresh.draw(&plan).unwrap();
        assert_eq!(pixels, fresh.readback().unwrap());
    }
}

#[test]
fn inset_shadow_is_clipped_to_element_and_complements_hole() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let bounds = Rect::from_edges(20.0, 20.0, 40.0, 40.0);
    let layer = ShadowLayer {
        dx: 0.0,
        dy: 2.0,
        blur: 0.0,
        spread: 0.0,
        inset: true,
        color: Oklch::BLACK,
    };
    let mut scene = Scene::new();
    scene.insert(shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(alpha(&pixels, 19, 20), 0);
    assert!(alpha(&pixels, 25, 20) > 240);
    assert_eq!(alpha(&pixels, 25, 25), 0);
}

#[test]
fn updating_background_preserves_inset_layer_during_partial_redraw() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let bounds = Rect::from_edges(20.0, 20.0, 40.0, 40.0);
    let background = |fill| DisplayItem::Rect {
        bounds,
        radius: 0.0,
        fill,
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    };
    let layer = ShadowLayer {
        dx: 0.0,
        dy: 2.0,
        blur: 0.0,
        spread: 0.0,
        inset: true,
        color: Oklch::BLACK,
    };
    let mut scene = Scene::new();
    let id = scene.insert(background(Oklch::WHITE));
    scene.insert(shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    scene.update(id, background(Oklch::from_hex("#ff0000").unwrap()));
    let plan = renderer.prepare(&mut scene);
    assert!(plan.partial);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let rim = &pixels[(20 * 80 + 25) * 4..][..4];
    let center = &pixels[(25 * 80 + 25) * 4..][..4];
    assert!(rim[0] < 10 && center[0] > 200);
}

#[test]
fn transformed_clipped_shadow_update_matches_full_redraw() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let clip = Rect::from_edges(10.0, 10.0, 65.0, 50.0);
    let bounds = Rect::from_edges(20.0, 20.0, 35.0, 35.0);
    let mut layer = ShadowLayer {
        dx: 1.0,
        dy: 1.0,
        blur: 6.0,
        spread: 3.0,
        inset: false,
        color: Oklch::BLACK,
    };
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushClip {
        bounds: clip,
        radius: 0.0,
    });
    scene.insert(DisplayItem::PushTransform(
        aqua_core::Transform2D::translate(8.0, 0.0),
    ));
    let id = scene.insert(shadow(bounds, layer));
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopClip);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    layer.dx = 9.0;
    scene.update(id, shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let updated = renderer.readback().unwrap();
    let Some(mut fresh) = new_renderer() else {
        return;
    };
    let mut fresh_scene = Scene::new();
    fresh_scene.insert(DisplayItem::PushClip {
        bounds: clip,
        radius: 0.0,
    });
    fresh_scene.insert(DisplayItem::PushTransform(
        aqua_core::Transform2D::translate(8.0, 0.0),
    ));
    fresh_scene.insert(shadow(bounds, layer));
    fresh_scene.insert(DisplayItem::PopTransform);
    fresh_scene.insert(DisplayItem::PopClip);
    let plan = fresh.prepare(&mut fresh_scene);
    fresh.draw(&plan).unwrap();
    assert_eq!(updated, fresh.readback().unwrap());
}
