use std::num::NonZeroU32;
use std::time::Duration;

use aqua_media::decode::animation::{
    decode_animation, AnimationBudget, AnimationFrame, AnimationLoops, DecodedAnimation,
};
use aqua_media::decode::{DecodedImage, MediaError};

fn frame(color: [u8; 4], milliseconds: u64) -> AnimationFrame {
    AnimationFrame {
        image: DecodedImage::from_rgba(1, 1, color.to_vec()).unwrap(),
        delay: Duration::from_millis(milliseconds),
    }
}

#[test]
fn timeline_handles_exact_boundaries_loops_skipped_frames_and_completion() {
    let animation = DecodedAnimation::new(
        vec![frame([255, 0, 0, 255], 20), frame([0, 255, 0, 255], 30)],
        AnimationLoops::Count(NonZeroU32::new(2).unwrap()),
    )
    .unwrap();
    let first = animation.sample(Duration::ZERO);
    assert_eq!(first.frame, 0);
    assert_eq!(first.next_frame_at, Some(Duration::from_millis(20)));
    assert_eq!(animation.sample(Duration::from_millis(20)).frame, 1);
    assert_eq!(animation.sample(Duration::from_millis(50)).frame, 0);
    assert_eq!(animation.sample(Duration::from_millis(91)).frame, 1);
    let complete = animation.sample(Duration::from_millis(100));
    assert_eq!(complete.frame, 1);
    assert!(complete.complete);
    assert_eq!(complete.next_frame_at, None);
    let looping = DecodedAnimation::new(
        vec![frame([255, 0, 0, 255], 20), frame([0, 255, 0, 255], 30)],
        AnimationLoops::Infinite,
    )
    .unwrap();
    assert_eq!(looping.sample(Duration::from_secs(1_000_000)).frame, 0);
    assert!(!looping.sample(Duration::from_secs(1_000_000)).complete);
}

#[test]
fn gif_and_webp_decoding_preserve_pixels_delays_and_enforce_retention_limits() {
    let mut encoded = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut encoded);
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
    let animation = decode_animation(&encoded, AnimationBudget::default()).unwrap();
    assert_eq!(animation.frames().len(), 2);
    assert_eq!(
        animation.frames()[0].image.pixel_at(0, 0),
        Some([255, 0, 0, 255])
    );
    assert_eq!(
        animation.frames()[1].image.pixel_at(0, 0),
        Some([0, 255, 0, 255])
    );
    assert_eq!(animation.frames()[0].delay, Duration::from_millis(30));
    assert!(matches!(
        decode_animation(
            &encoded,
            AnimationBudget {
                max_frames: 1,
                ..Default::default()
            }
        ),
        Err(MediaError::OverBudget)
    ));
    assert!(matches!(
        decode_animation(
            &encoded,
            AnimationBudget {
                max_bytes: 16,
                ..Default::default()
            }
        ),
        Err(MediaError::OverBudget)
    ));
    let mut encoded = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut encoded)
        .encode(&[17, 33, 65, 128], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let decoded = aqua_media::decode::decode_bytes(&encoded).unwrap();
    assert_eq!(decoded.pixel_at(0, 0), Some([17, 33, 65, 128]));
}
