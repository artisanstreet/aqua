use std::sync::Arc;
use std::time::{Duration, Instant};

use aqua::{ImageAssetBudget, ImageAssets, ImageStatus};
use aqua_core::ImageRef;
use aqua_render::media::{decode_svg, DecodedImage};

fn wait(handle: &aqua::ImageHandle) -> ImageStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = handle.status();
        if status != ImageStatus::Pending {
            return status;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn weak_lease_tracks_view_lifetime_cancellation_and_reused_generation() {
    let assets = ImageAssets::new(ImageAssetBudget {
        images: 1,
        ..Default::default()
    });
    let image = DecodedImage::from_rgba(2, 1, vec![255; 8]).unwrap();
    let handle = assets.insert(image.clone()).unwrap();
    let old_id = handle.id();
    let weak = handle.downgrade();
    let view: ImageRef = handle.clone().into();
    drop(handle);
    assert_eq!(weak.upgrade().unwrap().decoded_bytes(), Some(8));
    drop(view);
    assert!(weak.upgrade().is_none());
    let next = assets.insert(image).unwrap();
    assert_eq!(old_id.raw().index, next.id().raw().index);
    assert_ne!(old_id.raw().generation, next.id().raw().generation);
    assert!(weak.upgrade().is_none());
    let next_weak = next.downgrade();
    next.cancel();
    assert!(next_weak.upgrade().is_none());
    assert_eq!(next.decoded_bytes(), None);
}

#[test]
fn decoded_bytes_count_all_animation_frames() {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
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
    let image = assets.decode(Arc::<[u8]>::from(bytes)).unwrap();
    assert!(matches!(wait(&image), ImageStatus::Ready { .. }));
    assert_eq!(image.decoded_bytes(), Some(32));
}

#[test]
fn svg_decode_uses_supersampled_dimensions_and_enforces_budget() {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="2.25" height="1.5"><rect width="3" height="2" fill="#ff0000"/></svg>"##;
    let image = decode_svg(svg, 60).unwrap();
    assert_eq!((image.width(), image.height()), (4, 3));
    assert_eq!(image.pixel_at(0, 0), Some([255, 0, 0, 255]));
    assert!(decode_svg(svg, 59).is_err());
    let assets = ImageAssets::default();
    let image = assets.decode(Arc::<[u8]>::from(svg.as_slice())).unwrap();
    assert_eq!(
        wait(&image),
        ImageStatus::Ready {
            width: 4,
            height: 3
        }
    );
    assert_eq!(image.decoded_bytes(), Some(48));
}

#[test]
fn preview_resizes_still_images_and_rejects_more_than_120_gif_frames() {
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1921,
        1,
        image::Rgba([1, 2, 3, 255]),
    ))
    .write_to(&mut png, image::ImageFormat::Png)
    .unwrap();
    let assets = ImageAssets::default();
    let image = assets
        .decode_preview(Arc::<[u8]>::from(png.into_inner()))
        .unwrap();
    assert_eq!(
        wait(&image),
        ImageStatus::Ready {
            width: 1920,
            height: 1
        }
    );
    assert_eq!(image.decoded_bytes(), Some(1920 * 4));

    let mut gif = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
        for _ in 0..121 {
            encoder
                .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                    1,
                    1,
                    image::Rgba([0, 0, 0, 255]),
                )))
                .unwrap();
        }
    }
    let image = assets.decode_preview(Arc::<[u8]>::from(gif)).unwrap();
    assert!(matches!(wait(&image), ImageStatus::Failed(_)));
    assert_eq!(image.decoded_bytes(), None);
}

#[test]
fn preview_gif_keeps_authored_delay_and_loops_forever() {
    use aqua_render::media::animation::{decode_preview_gif, AnimationLoops};
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Finite(1))
            .unwrap();
        for delay in [0, 20] {
            encoder
                .encode_frame(image::Frame::from_parts(
                    image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255])),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(delay, 1),
                ))
                .unwrap();
        }
    }
    let animation = decode_preview_gif(&bytes, 64).unwrap();
    assert_eq!(animation.loops(), AnimationLoops::Infinite);
    assert_eq!(animation.frames()[0].delay, Duration::ZERO);
    assert_eq!(animation.frames()[1].delay, Duration::from_millis(20));
}
