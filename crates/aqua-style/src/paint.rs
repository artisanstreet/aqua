use aqua_core::{Oklch, Srgba8};

use crate::Paint;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum PaintValue {
    #[default]
    Empty,
    Solid(Oklch),
    Srgba(Srgba8),
    Styled(Box<Paint>),
}

impl From<Paint> for PaintValue {
    #[inline]
    fn from(paint: Paint) -> Self {
        match paint
            == (Paint {
                fill: paint.fill,
                ..Paint::default()
            }) {
            true => match paint.fill {
                Some(fill) => Self::Solid(fill),
                None => Self::Empty,
            },
            false => Self::Styled(Box::new(paint)),
        }
    }
}

impl PaintValue {
    #[inline]
    pub fn is_plain(&self) -> bool {
        matches!(self, Self::Empty | Self::Solid(_) | Self::Srgba(_))
    }

    #[inline]
    pub fn fill(&self) -> Option<Oklch> {
        match self {
            Self::Empty => None,
            Self::Solid(fill) => Some(*fill),
            Self::Srgba(fill) => Some(fill.to_oklch()),
            Self::Styled(paint) => paint.fill,
        }
    }

    #[inline]
    pub fn set_fill(&mut self, fill: Option<Oklch>) -> bool {
        if self.fill() == fill {
            return false;
        }
        match self {
            Self::Styled(paint) => paint.fill = fill,
            _ => {
                *self = match fill {
                    Some(fill) => Self::Solid(fill),
                    None => Self::Empty,
                }
            }
        }
        true
    }

    #[inline]
    pub fn srgba(&self) -> Option<Srgba8> {
        match self {
            Self::Srgba(fill) => Some(*fill),
            _ => None,
        }
    }

    #[inline]
    pub fn set_srgba(&mut self, fill: Srgba8) -> bool {
        match self {
            Self::Srgba(previous) if *previous == fill => false,
            Self::Styled(_) => self.set_fill(Some(fill.to_oklch())),
            _ => {
                *self = Self::Srgba(fill);
                true
            }
        }
    }

    #[inline]
    pub fn resolve(&self) -> Paint {
        match self {
            Self::Empty => Paint::default(),
            Self::Solid(fill) => Paint {
                fill: Some(*fill),
                ..Paint::default()
            },
            Self::Srgba(fill) => Paint {
                fill: Some(fill.to_oklch()),
                ..Paint::default()
            },
            Self::Styled(paint) => paint.as_ref().clone(),
        }
    }
}

impl From<PaintValue> for Paint {
    fn from(value: PaintValue) -> Self {
        match value {
            PaintValue::Empty => Self::default(),
            PaintValue::Solid(fill) => Self {
                fill: Some(fill),
                ..Self::default()
            },
            PaintValue::Srgba(fill) => Self {
                fill: Some(fill.to_oklch()),
                ..Self::default()
            },
            PaintValue::Styled(paint) => *paint,
        }
    }
}
