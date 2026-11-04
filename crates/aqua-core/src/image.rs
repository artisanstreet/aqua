use crate::{Point, Rect, Size};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ImageFit {
    #[default]
    Fill,
    Contain,
    Cover,
    None,
    ScaleDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFitError {
    InvalidBounds,
    InvalidSource,
    InvalidPosition,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePlacement {
    pub destination: Rect,
    pub source: Rect,
}

impl ImagePlacement {
    pub fn resolve(
        bounds: Rect,
        natural: Size,
        fit: ImageFit,
        position: Point,
        source: Rect,
    ) -> Result<Self, ImageFitError> {
        if ![bounds.x, bounds.y, bounds.w, bounds.h, natural.w, natural.h]
            .iter()
            .all(|value| value.is_finite())
            || bounds.w <= 0.0
            || bounds.h <= 0.0
            || natural.w <= 0.0
            || natural.h <= 0.0
        {
            return Err(ImageFitError::InvalidBounds);
        }
        if ![source.x, source.y, source.w, source.h]
            .iter()
            .all(|value| value.is_finite())
            || source.x < 0.0
            || source.y < 0.0
            || source.w <= 0.0
            || source.h <= 0.0
            || source.right() > 1.0
            || source.bottom() > 1.0
        {
            return Err(ImageFitError::InvalidSource);
        }
        if !position.x.is_finite() || !position.y.is_finite() {
            return Err(ImageFitError::InvalidPosition);
        }
        let width = natural.w * source.w;
        let height = natural.h * source.h;
        let contain = (bounds.w / width).min(bounds.h / height);
        let scale = match fit {
            ImageFit::Fill => {
                return Ok(Self {
                    destination: bounds,
                    source,
                });
            }
            ImageFit::Contain => contain,
            ImageFit::Cover => (bounds.w / width).max(bounds.h / height),
            ImageFit::None => 1.0,
            ImageFit::ScaleDown => contain.min(1.0),
        };
        let width = width * scale;
        let height = height * scale;
        if !width.is_finite() || !height.is_finite() {
            return Err(ImageFitError::InvalidBounds);
        }
        Ok(Self {
            destination: Rect {
                x: bounds.x + (bounds.w - width) * position.x,
                y: bounds.y + (bounds.h - height) * position.y,
                w: width,
                h: height,
            },
            source,
        })
    }
}

#[derive(Clone)]
pub struct ImageRef {
    id: crate::ImageId,
    lease: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

impl ImageRef {
    pub fn with_lease(
        id: crate::ImageId,
        lease: std::sync::Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self {
            id,
            lease: Some(lease),
        }
    }
    pub fn id(&self) -> crate::ImageId {
        self.id
    }
}

impl From<crate::ImageId> for ImageRef {
    fn from(id: crate::ImageId) -> Self {
        Self { id, lease: None }
    }
}

impl std::fmt::Debug for ImageRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("ImageRef").field(&self.id).finish()
    }
}

impl PartialEq for ImageRef {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && match (&self.lease, &other.lease) {
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for ImageRef {}
