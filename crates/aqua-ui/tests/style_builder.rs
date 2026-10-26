//! Style-builder integration tests (style-only surface).
//!
//! These exercise the canonical `Style` API exactly as peer crates
//! (element/facade) consume it: builder chains, ordered compose over
//! fixed arrays, `apply_to` onto the resolved pair, and typography
//! metadata for element inheritance.

use aqua_core::{Insets, Oklch};
use aqua_ui::style::{auto, pct, px, LayoutStyle, Paint, Style, Typography};

#[test]
fn agreed_chain_compiles_and_resolves() {
    let s = Style::new()
        .width(px(120.0))
        .height(pct(50.0))
        .padding(px(8.0))
        .radius(px(6.0))
        .background(Oklch::WHITE)
        .text_color(Oklch::BLACK);
    let mut layout = LayoutStyle::default();
    let mut paint = Paint::default();
    s.apply_to(&mut layout, &mut paint);
    assert_eq!(layout.padding, Insets::uniform(8.0));
    assert_eq!(paint.radius, 6.0);
    assert_eq!(paint.fill, Some(Oklch::WHITE));
}

#[test]
fn fixed_array_compose_later_wins_per_field() {
    let theme = Style::new()
        .background(Oklch::WHITE)
        .font_size(12.0)
        .gap(4.0);
    let local = Style::new().font_size(18.0);
    let clear = Style::new().clear_background();
    // Borrowed fixed array — the documented compose shape.
    let arr = [theme, local, clear];
    let composed = Style::compose(&arr);
    let typo: Typography = composed.resolve_typography();
    assert_eq!(typo.font_size, 18.0);
    assert_eq!(composed.resolve_paint().fill, None);
    assert_eq!(composed.resolve_layout().gap, 4.0);
}

#[test]
fn apply_to_preserves_unset_targets() {
    let mut layout = LayoutStyle {
        gap: 11.0,
        ..Default::default()
    };
    let mut paint = Paint {
        radius: 3.0,
        ..Default::default()
    };
    Style::new()
        .width(px(64.0))
        .apply_to(&mut layout, &mut paint);
    assert_eq!(layout.gap, 11.0);
    assert_eq!(paint.radius, 3.0);
}

#[test]
fn typography_defaults_match_textcontent_label() {
    let t = Style::new().resolve_typography();
    assert_eq!(t.font_size, 14.0);
    assert_eq!(t.color, Oklch::WHITE);
    assert!(!t.wrap);

    // apply_typography targets tree TextContent-shaped fields.
    let s = Style::new().font_size(20.0).text_wrap(true);
    let (mut fs, mut color, mut wrap) = (14.0, Oklch::WHITE, false);
    s.apply_typography(&mut fs, &mut color, &mut wrap);
    assert_eq!((fs, wrap), (20.0, true));
    assert_eq!(color, Oklch::WHITE); // unset -> preserved
}

#[test]
fn widths_accept_px_pct_and_auto_lengths() {
    use aqua_ui::style::Length;
    let a = Style::new().width(px(10.0)).resolve_layout();
    assert_eq!(a.width, Length::Px(10.0));
    let b = Style::new().width(pct(50.0)).resolve_layout();
    assert_eq!(b.width, Length::Pct(50.0));
    let c = Style::new().width(auto()).resolve_layout();
    assert_eq!(c.width, Length::Auto);
    // Bare numbers are NOT accepted: exact Length keeps units unambiguous
    // (element builders keep their own Into-based ergonomics).
}

#[test]
fn merge_with_builder_form_chains() {
    let s = Style::new()
        .width(px(50.0))
        .with(&Style::new().height(px(60.0)))
        .with(&Style::new().clip(true));
    let layout = s.resolve_layout();
    assert_eq!(layout.height, aqua_ui::style::Length::Px(60.0));
    assert!(s.resolve_paint().clip);
}
