use std::collections::BTreeMap;

use aqua_motion::{Easing, MotionKind, SpringParams};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion(MotionKind);

impl Motion {
    pub const INSTANT: Motion = Motion(MotionKind::Instant);

    #[must_use]
    pub const fn spring(params: SpringParams) -> Self {
        Self(MotionKind::Spring(params))
    }

    #[must_use]
    pub const fn tween(duration_secs: f64, easing: Easing) -> Self {
        Self(MotionKind::Tween {
            duration_secs,
            easing,
        })
    }

    #[must_use]
    pub const fn kind(self) -> MotionKind {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MotionToken(&'static str);

impl MotionToken {
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionRef {
    Literal(Motion),
    Token(MotionToken),
}

impl From<Motion> for MotionRef {
    fn from(motion: Motion) -> Self {
        Self::Literal(motion)
    }
}

impl From<MotionToken> for MotionRef {
    fn from(token: MotionToken) -> Self {
        Self::Token(token)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MotionTheme {
    fallback: Motion,
    tokens: BTreeMap<MotionToken, Motion>,
}

impl Default for MotionTheme {
    fn default() -> Self {
        Self::new(Motion::INSTANT)
    }
}

impl MotionTheme {
    #[must_use]
    pub fn new(fallback: Motion) -> Self {
        Self {
            fallback,
            tokens: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with(mut self, token: MotionToken, motion: Motion) -> Self {
        self.tokens.insert(token, motion);
        self
    }

    #[must_use]
    pub fn resolve(&self, motion: MotionRef) -> Motion {
        match motion {
            MotionRef::Literal(motion) => motion,
            MotionRef::Token(token) => self.tokens.get(&token).copied().unwrap_or(self.fallback),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AnimatedProperty {
    Opacity,
    Fill,
    BorderColor,
    TextColor,
    Translate,
    Frame,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Animate {
    opacity: Option<MotionRef>,
    fill: Option<MotionRef>,
    border_color: Option<MotionRef>,
    text_color: Option<MotionRef>,
    translate: Option<MotionRef>,
    frame: Option<MotionRef>,
}

impl Animate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn opacity(mut self, motion: impl Into<MotionRef>) -> Self {
        self.opacity = Some(motion.into());
        self
    }

    #[must_use]
    pub fn fill(mut self, motion: impl Into<MotionRef>) -> Self {
        self.fill = Some(motion.into());
        self
    }

    #[must_use]
    pub fn border_color(mut self, motion: impl Into<MotionRef>) -> Self {
        self.border_color = Some(motion.into());
        self
    }

    #[must_use]
    pub fn text_color(mut self, motion: impl Into<MotionRef>) -> Self {
        self.text_color = Some(motion.into());
        self
    }

    #[must_use]
    pub fn translate(mut self, motion: impl Into<MotionRef>) -> Self {
        self.translate = Some(motion.into());
        self
    }

    #[must_use]
    pub fn frame(mut self, motion: impl Into<MotionRef>) -> Self {
        self.frame = Some(motion.into());
        self
    }

    #[must_use]
    pub fn motion_of(&self, property: AnimatedProperty) -> Option<MotionRef> {
        match property {
            AnimatedProperty::Opacity => self.opacity,
            AnimatedProperty::Fill => self.fill,
            AnimatedProperty::BorderColor => self.border_color,
            AnimatedProperty::TextColor => self.text_color,
            AnimatedProperty::Translate => self.translate,
            AnimatedProperty::Frame => self.frame,
        }
    }

    #[must_use]
    pub fn properties(&self) -> Vec<(AnimatedProperty, MotionRef)> {
        [
            AnimatedProperty::Opacity,
            AnimatedProperty::Fill,
            AnimatedProperty::BorderColor,
            AnimatedProperty::TextColor,
            AnimatedProperty::Translate,
            AnimatedProperty::Frame,
        ]
        .into_iter()
        .filter_map(|property| self.motion_of(property).map(|motion| (property, motion)))
        .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Presence {
    enter: Option<MotionRef>,
    exit: Option<MotionRef>,
    slide: Option<(f32, f32)>,
}

impl Presence {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn enter(mut self, motion: impl Into<MotionRef>) -> Self {
        self.enter = Some(motion.into());
        self
    }

    #[must_use]
    pub fn exit(mut self, motion: impl Into<MotionRef>) -> Self {
        self.exit = Some(motion.into());
        self
    }

    #[must_use]
    pub fn slide(mut self, dx: f32, dy: f32) -> Self {
        self.slide = Some((dx, dy));
        self
    }

    #[must_use]
    pub fn slide_offset(&self) -> Option<(f32, f32)> {
        self.slide
    }

    #[must_use]
    pub fn enter_motion(&self) -> Option<MotionRef> {
        self.enter
    }

    #[must_use]
    pub fn exit_motion(&self) -> Option<MotionRef> {
        self.exit
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct MotionSpec {
    pub animate: Animate,
    pub presence: Presence,
}

impl MotionSpec {
    #[must_use]
    pub fn with_animate(self: &std::rc::Rc<Self>, animate: Animate) -> std::rc::Rc<Self> {
        std::rc::Rc::new(Self {
            animate,
            presence: self.presence,
        })
    }

    #[must_use]
    pub fn with_presence(self: &std::rc::Rc<Self>, presence: Presence) -> std::rc::Rc<Self> {
        std::rc::Rc::new(Self {
            animate: self.animate.clone(),
            presence,
        })
    }
}
