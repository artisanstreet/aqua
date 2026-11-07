use aqua_core::Oklch;

use crate::Paint;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum PaintValue {
    #[default]
    Empty,
    Solid(Oklch),
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
        matches!(self, Self::Empty | Self::Solid(_))
    }

    #[inline]
    pub fn fill(&self) -> Option<Oklch> {
        match self {
            Self::Empty => None,
            Self::Solid(fill) => Some(*fill),
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
    pub fn resolve(&self) -> Paint {
        match self {
            Self::Empty => Paint::default(),
            Self::Solid(fill) => Paint {
                fill: Some(*fill),
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
            PaintValue::Styled(paint) => *paint,
        }
    }
}
