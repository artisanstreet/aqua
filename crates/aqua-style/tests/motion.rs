use std::rc::Rc;

use aqua_motion::{Easing, MotionKind, SpringParams};
use aqua_style::motion::{
    AnimatedProperty, Animate, Motion, MotionRef, MotionSpec, MotionTheme, MotionToken, Presence,
};

const SLOW: MotionToken = MotionToken::new("slow");
const FADE: Motion = Motion::tween(0.12, Easing::EaseOut);
const CARD: Motion = Motion::spring(SpringParams::SNAPPY);

#[test]
fn tokens_resolve_through_the_theme_and_literals_pass_through() {
    let theme = MotionTheme::new(Motion::INSTANT).with(SLOW, CARD);
    assert_eq!(theme.resolve(SLOW.into()), CARD);
    assert_eq!(theme.resolve(FADE.into()), FADE);
}

#[test]
fn unknown_tokens_use_the_fallback() {
    let theme = MotionTheme::new(FADE);
    assert_eq!(theme.resolve(SLOW.into()), FADE);
    assert_eq!(
        theme.resolve(SLOW.into()).kind(),
        MotionKind::Tween {
            duration_secs: 0.12,
            easing: Easing::EaseOut
        }
    );
}

#[test]
fn retuning_a_token_changes_every_reference() {
    let before = MotionTheme::default().with(SLOW, CARD);
    let after = before.clone().with(SLOW, FADE);
    assert_ne!(before.resolve(SLOW.into()), after.resolve(SLOW.into()));
    assert_ne!(before, after);
}

#[test]
fn animate_lists_only_declared_properties() {
    let animate = Animate::new().fill(CARD).frame(SLOW);
    let declared: Vec<AnimatedProperty> = animate
        .properties()
        .into_iter()
        .map(|(property, _)| property)
        .collect();
    assert_eq!(declared, vec![AnimatedProperty::Fill, AnimatedProperty::Frame]);
    assert_eq!(
        animate.motion_of(AnimatedProperty::Frame),
        Some(MotionRef::Token(SLOW))
    );
    assert_eq!(animate.motion_of(AnimatedProperty::Opacity), None);
}

#[test]
fn replacing_one_half_of_a_spec_keeps_the_other() {
    let spec = Rc::new(MotionSpec::default());
    let spec = spec.with_presence(Presence::new().exit(FADE));
    let spec = spec.with_animate(Animate::new().opacity(CARD));
    assert_eq!(spec.presence.exit_motion(), Some(FADE.into()));
    assert_eq!(
        spec.animate.motion_of(AnimatedProperty::Opacity),
        Some(CARD.into())
    );
}
