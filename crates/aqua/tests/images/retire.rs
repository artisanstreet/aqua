use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{ImageAssetBudget, ImageAssets, ImageStatus, TEST_SERIAL};

fn png(side: u32) -> Arc<[u8]> {
    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(side, side, image::Rgba([10, 20, 30, 255]))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    Arc::from(bytes)
}

fn settle(assets: &ImageAssets, id: aqua_core::ImageId, ready: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while assets.ready(id).is_some() != ready {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn retired_image_keeps_status_and_frees_decoded_bytes_until_revived() {
    let _serial = TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let assets = ImageAssets::new(ImageAssetBudget::editor());
    let handle = assets.decode_preview(png(600)).unwrap();
    settle(&assets, handle.id(), true);
    let decoded = 600 * 600 * 4;
    assert_eq!(assets.stats().decoded_bytes, decoded);

    assets.retire(handle.id());
    assert_eq!(assets.stats().decoded_bytes, 0);
    assert!(assets.ready(handle.id()).is_none());
    assert!(assets.has_data(handle.id()));
    assert_eq!(
        handle.status(),
        ImageStatus::Ready {
            width: 600,
            height: 600
        }
    );
    assert_eq!(handle.decoded_bytes(), Some(decoded));

    assets.revive(handle.id());
    assert_eq!(
        handle.status(),
        ImageStatus::Ready {
            width: 600,
            height: 600
        }
    );
    settle(&assets, handle.id(), true);
    assert_eq!(assets.stats().decoded_bytes, decoded);
}

#[test]
fn small_images_are_never_retired() {
    let _serial = TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let assets = ImageAssets::new(ImageAssetBudget::editor());
    let handle = assets.decode_preview(png(32)).unwrap();
    settle(&assets, handle.id(), true);
    assets.retire(handle.id());
    assert!(assets.ready(handle.id()).is_some());
}

#[test]
fn idle_retirement_spares_kept_images_and_young_images() {
    let _serial = TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let assets = ImageAssets::new(ImageAssetBudget::editor());
    let kept = assets.decode_preview(png(600)).unwrap();
    let unused = assets.decode_preview(png(600)).unwrap();
    settle(&assets, kept.id(), true);
    settle(&assets, unused.id(), true);
    let keep = std::collections::HashSet::from([kept.id()]);

    assets.retire_idle(&keep, Duration::from_secs(3600));
    assert!(assets.ready(kept.id()).is_some() && assets.ready(unused.id()).is_some());

    assets.retire_idle(&keep, Duration::ZERO);
    assert!(assets.ready(kept.id()).is_some());
    assert!(assets.ready(unused.id()).is_none());
    assert!(assets.has_data(unused.id()));
    assert_eq!(assets.stats().decoded_bytes, 600 * 600 * 4);
}
