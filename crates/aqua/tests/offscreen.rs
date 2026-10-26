//! Verification: offscreen GPU composition through the public render API.
//!
//! Renders a real scene with `Renderer::headless` (software Vulkan lavapipe
//! where available) and reads back pixels. Skips with an explicit message
//! when no adapter exists — never fake-passes. Assertions use the public
//! diagnostics contract: `nodes_total` vs `nodes_updated`, exact upload
//! bytes (local update << full-scene upload), and pixel checks.

use aqua_render::{DisplayItem, Oklch, Rect, Renderer, Scene};

fn test_red() -> Oklch {
    Oklch::from_hex("#ff0000").unwrap()
}

fn headless_or_skip(w: u32, h: u32) -> Option<Renderer> {
    match pollster::block_on(Renderer::headless(w, h)) {
        Ok(r) => Some(r),
        // Strict verification: ONLY a missing adapter may skip, and only
        // when AQUA_REQUIRE_GPU!=1. Every other init error panics.
        // (Never `Err(_) => SKIP`.) The SKIP-NO-ADAPTER marker lets the
        // canonical scripts report an honest SKIP instead of a pass.
        Err(e) => {
            let strict = std::env::var("AQUA_REQUIRE_GPU").is_ok_and(|v| v == "1");
            if matches!(e, aqua_render::RenderError::AdapterNotFound(_)) && !strict {
                eprintln!("SKIP-NO-ADAPTER offscreen GPU test (no adapter): {e}");
                return None;
            }
            panic!("offscreen GPU init failed (must not skip): {e:?}");
        }
    }
}

fn rect_at(x: f32, y: f32, fill: Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x,
            y,
            w: 14.0,
            h: 14.0,
        },
        radius: 2.0,
        fill,
        border_width: 0.0,
        border_color: fill,
        shadow: None,
    }
}

#[test]
fn offscreen_red_rect_pixel_check() {
    let Some(mut r) = headless_or_skip(128, 128) else {
        return;
    };
    let mut scene = Scene::new();
    let red = test_red();
    scene.insert(DisplayItem::Rect {
        bounds: Rect {
            x: 8.0,
            y: 8.0,
            w: 112.0,
            h: 112.0,
        },
        radius: 0.0,
        fill: red,
        border_width: 0.0,
        border_color: red,
        shadow: None,
    });
    let plan = r.prepare(&mut scene);
    assert!(!plan.is_clean(), "new content must produce work");
    r.draw(&plan).expect("draw");
    let d = r.diagnostics();
    assert_eq!(
        d.frames_presented, 0,
        "offscreen draws never count as presented"
    );
    assert_eq!(d.nodes_total_last, 1);
    assert_eq!(d.nodes_updated_last, 1);
    assert!(d.bytes_uploaded_last > 0);

    let px = r.readback().expect("readback");
    let c = &px[(64 * 128 + 64) * 4..][..4];
    assert!(c[0] > 200, "red channel: {c:?}");
    assert!(c[1] < 30 && c[2] < 30, "green/blue near zero: {c:?}");
    assert_eq!(c[3], 255, "opaque: {c:?}");
    assert_eq!(&px[0..4], &[0, 0, 0, 0], "clear color outside rect");
}

#[test]
fn offscreen_trivial_update_uploads_partial_only() {
    let Some(mut r) = headless_or_skip(128, 128) else {
        return;
    };
    let mut scene = Scene::new();
    let red = test_red();
    let blue = Oklch::from_hex("#0000ff").unwrap();
    let mut ids = Vec::new();
    for i in 0..64 {
        ids.push(scene.insert(rect_at((i % 8) as f32 * 16.0, (i / 8) as f32 * 16.0, red)));
    }
    let plan = r.prepare(&mut scene);
    r.draw(&plan).expect("draw");
    let full_bytes = r.diagnostics().bytes_uploaded_last;
    assert!(full_bytes > 0);

    scene.update(ids[30], rect_at(6.0 * 16.0, 3.0 * 16.0, blue));
    let plan2 = r.prepare(&mut scene);
    assert_eq!(plan2.nodes_total, plan.nodes_total);
    assert_eq!(plan2.nodes_updated, 1, "exactly one change drained");
    r.draw(&plan2).expect("draw");
    let partial_bytes = r.diagnostics().bytes_uploaded_last;
    assert!(
        partial_bytes < full_bytes,
        "partial upload {partial_bytes} < full {full_bytes}"
    );
    let px = r.readback().expect("readback");
    let (cx, cy) = (6 * 16 + 7, 3 * 16 + 7);
    let c = &px[(cy * 128 + cx) * 4..][..4];
    assert!(c[2] > 200 && c[0] < 30, "blue after update: {c:?}");
}

#[test]
fn offscreen_idle_frame_is_clean() {
    let Some(mut r) = headless_or_skip(64, 64) else {
        return;
    };
    let mut scene = Scene::new();
    scene.insert(rect_at(4.0, 4.0, test_red()));
    let plan = r.prepare(&mut scene);
    r.draw(&plan).expect("draw");
    let presented = r.diagnostics().frames_presented;
    // No changes: the plan is clean and the facade must skip presentation.
    let idle = r.prepare(&mut scene);
    assert!(idle.is_clean(), "no changes means no work");
    assert_eq!(
        r.diagnostics().frames_presented,
        presented,
        "verification never presents a clean plan"
    );
}
