use std::sync::Arc;
use std::time::{Duration, Instant};

use aqua::{ImageAssetBudget, ImageAssetError, ImageAssets, ImageStatus};
use aqua_core::ImageRef;
use aqua_render::media::DecodedImage;

fn image() -> DecodedImage {
    DecodedImage::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap()
}

#[test]
fn leases_retain_decoded_data_until_last_reference_and_generations_reject_stale_ids() {
    let assets = ImageAssets::new(ImageAssetBudget {
        decoded_bytes: 8,
        images: 1,
        ..Default::default()
    });
    let first = assets.insert(image()).unwrap();
    let previous = first.id();
    let lease: ImageRef = first.clone().into();
    drop(first);
    assert_eq!(assets.stats().decoded_bytes, 8);
    assert!(matches!(
        assets.insert(image()),
        Err(ImageAssetError::OverBudget)
    ));
    drop(lease);
    assert_eq!(assets.stats().decoded_bytes, 0);
    let next = assets.insert(image()).unwrap();
    assert_eq!(next.id().raw().index, previous.raw().index);
    assert_ne!(next.id().raw().generation, previous.raw().generation);
    let other = ImageAssets::default().insert(image()).unwrap();
    assert_ne!(other.id(), next.id());
    next.cancel();
    assert_eq!(next.status(), ImageStatus::Cancelled);
    assert_eq!(assets.stats().images, 0);
}

#[test]
fn async_cancellation_cannot_publish_into_reused_generation_and_errors_release_admission() {
    let assets = ImageAssets::new(ImageAssetBudget {
        images: 1,
        workers: 1,
        pending_bytes: 1024,
        ..Default::default()
    });
    let invalid = assets.decode(Arc::<[u8]>::from([1, 2, 3, 4])).unwrap();
    let previous = invalid.id();
    invalid.cancel();
    let next = assets.insert(image()).unwrap();
    assert_eq!(next.id().raw().index, previous.raw().index);
    let deadline = Instant::now() + Duration::from_secs(5);
    while assets.stats().workers > 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        next.status(),
        ImageStatus::Ready {
            width: 2,
            height: 1
        }
    );
    assert_eq!(assets.stats().pending_bytes, 0);
    assert_eq!(assets.stats().decoded_bytes, 8);
    next.cancel();
    let malformed = assets.decode(Arc::<[u8]>::from([1, 2, 3, 4])).unwrap();
    while matches!(malformed.status(), ImageStatus::Pending) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(
        malformed.status(),
        ImageStatus::Failed(ImageAssetError::Decode(_))
    ));
    assert!(matches!(
        assets.decode(Arc::<[u8]>::from(vec![0; 1025])),
        Err(ImageAssetError::OverBudget)
    ));
    assert_eq!(assets.stats().workers, 0);
    assert_eq!(assets.stats().pending_bytes, 0);
}

#[test]
fn status_notifications_publish_on_owner_thread_and_disposed_scopes_ignore_late_updates() {
    let assets = ImageAssets::default();
    let runtime = aqua_reactive::Runtime::new();
    let scope = runtime.create_scope();
    let handle = assets.insert(image()).unwrap();
    let status = assets.status(&scope, &handle);
    assert_eq!(
        status.get(),
        ImageStatus::Ready {
            width: 2,
            height: 1
        }
    );
    handle.cancel();
    assets.flush_notifications();
    assert_eq!(status.get(), ImageStatus::Cancelled);
    scope.dispose();
    let _next = assets.insert(image()).unwrap();
    assets.flush_notifications();
    assert!(!status.is_live());
}

#[test]
fn canonical_context_image_lease_disposes_with_the_headless_application() {
    let retained = std::cell::RefCell::new(None);
    aqua::App::new()
        .config(aqua::Config::new().headless(true))
        .run(|context| {
            let assets = context.images();
            let image = assets.insert(image()).unwrap();
            *retained.borrow_mut() = Some(assets);
            aqua::ui::Media::new(image)
                .style(
                    aqua::Style::new()
                        .width(aqua::px(32.0))
                        .height(aqua::px(16.0)),
                )
                .build()
        })
        .unwrap();
    assert_eq!(retained.borrow().as_ref().unwrap().stats().decoded_bytes, 0);
}

#[test]
fn batches_queue_within_byte_budget_and_cancel_without_exceeding_worker_limit() {
    let assets = ImageAssets::new(ImageAssetBudget {
        workers: 1,
        images: 128,
        pending_bytes: 512,
        ..Default::default()
    });
    let mut handles = Vec::new();
    for _ in 0..100 {
        handles.push(assets.decode(Arc::<[u8]>::from([1, 2, 3, 4])).unwrap());
        assert!(assets.stats().workers <= 1);
        assert!(assets.stats().pending_bytes <= 512);
    }
    for handle in handles.iter().step_by(2) {
        handle.cancel();
    }
    let timeout = Instant::now() + Duration::from_secs(5);
    while assets.stats().workers > 0 {
        assert!(Instant::now() < timeout);
        std::thread::sleep(Duration::from_millis(1));
    }
    for (index, handle) in handles.iter().enumerate() {
        if index % 2 == 0 {
            assert_eq!(handle.status(), ImageStatus::Cancelled);
        } else {
            assert!(matches!(
                handle.status(),
                ImageStatus::Failed(ImageAssetError::Decode(_))
            ));
        }
    }
    assert_eq!(assets.stats().pending_bytes, 0);
    drop(handles);
    assert_eq!(assets.stats().images, 0);
}

#[test]
fn shared_assets_notify_each_live_subscriber_and_remove_closed_hosts() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let assets = ImageAssets::default();
    let first = Arc::new(AtomicUsize::new(0));
    let second = Arc::new(AtomicUsize::new(0));
    let count = first.clone();
    let a = assets.subscribe(Arc::new(move || {
        count.fetch_add(1, Ordering::Relaxed);
    }));
    let count = second.clone();
    let b = assets.subscribe(Arc::new(move || {
        count.fetch_add(1, Ordering::Relaxed);
    }));
    let handle = assets.insert(image()).unwrap();
    assert_eq!(first.load(Ordering::Relaxed), 1);
    assert_eq!(second.load(Ordering::Relaxed), 1);
    drop(a);
    handle.cancel();
    assert_eq!(first.load(Ordering::Relaxed), 1);
    assert_eq!(second.load(Ordering::Relaxed), 2);
    drop(b);
    let _next = assets.insert(image()).unwrap();
    assert_eq!(first.load(Ordering::Relaxed), 1);
    assert_eq!(second.load(Ordering::Relaxed), 2);
}
