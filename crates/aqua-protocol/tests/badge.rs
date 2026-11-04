use aqua_core::Oklch;
use aqua_protocol::badge::TaskbarBadge;

#[test]
fn badge_composition_preserves_circle_alpha_and_selects_contrasting_ink() {
    for (color, ink) in [("#ffffff", 0), ("#000000", 255)] {
        let badge = TaskbarBadge {
            count: 3,
            color: Oklch::from_hex(color).unwrap(),
        };
        let mut coverage = vec![0; 32 * 32];
        coverage[16 * 32 + 16] = 255;
        let pixels = badge.compose(32, &coverage).unwrap();
        assert_eq!(
            &pixels[(16 * 32 + 16) * 4..(16 * 32 + 16) * 4 + 4],
            &[ink, ink, ink, 255]
        );
        assert_eq!(pixels[3], 0);
        assert!(pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] > 0 && pixel[3] < 255));
    }
}

#[test]
fn badge_rejects_malformed_masks_and_bounds_allocation() {
    let badge = TaskbarBadge {
        count: 12,
        color: Oklch::from_hex("#ff0000").unwrap(),
    };
    assert!(badge.compose(32, &[255]).is_none());
    assert!(badge.compose(u32::MAX, &[]).is_none());
}
