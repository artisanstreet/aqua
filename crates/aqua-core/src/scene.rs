//! Scene op vocabulary: the retained paint operations `aqua-ui` commits and
//! `aqua-render` consumes, in painter order.
//!
//! This module defines vocabulary ONLY. The retained incremental store
//! (`insert`/`update`/`remove`, dirty queues, damage) is owned by
//! `aqua-render` and stays there — there is exactly one scene store.
//!
//! Conventions (binding):
//! - Geometry is window-local logical units; the renderer scales to physical.
//! - `BackdropBlur.radius` is in physical pixels at scale 1; renderer scales.
//! - Paint ops never carry GPU handles: images/viewports travel as opaque
//!   `layer` keys namespaced by the media owner.

use crate::color::Oklch;
use crate::geometry::Rect;
use crate::id::{ImageId, RawId, ViewportId};

/// Layer-namespace tags for `DisplayItem::External.layer` (contract §4).
/// The top two bits disambiguate media frames (`01`) from viewport frames
/// (`10`); the low 62 bits carry `(generation << 32 | index)` with full
/// 32-bit slot indices and 30-bit generations — no silent truncation.
///
/// Capacity contract (binding): handle stores (image slots, viewport slots)
/// MUST retire a slot before its generation exceeds [`LAYER_MAX_GENERATION`]
/// (2^30 − 1 removes of one slot), exactly like the arena's wrap-retirement.
/// The encoders below `assert!` instead of masking: an over-bound generation
/// fails closed rather than aliasing a stale frame.
pub const LAYER_TAG_MEDIA: u64 = 0x4000_0000_0000_0000;
/// Layer-namespace tag for viewport frames, disjoint from media by the top
/// two bits.
pub const LAYER_TAG_VIEWPORT: u64 = 0x8000_0000_0000_0000;
/// Mask selecting the two namespace tag bits.
pub const LAYER_TAG_MASK: u64 = 0xC000_0000_0000_0000;
/// Largest encodable generation (30 bits). Stores retire slots at this bound.
pub const LAYER_MAX_GENERATION: u32 = 0x3FFF_FFFF;

/// Which namespace a layer key belongs to (see [`decode_layer`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    /// Image/media frame.
    Media,
    /// Viewport frame.
    Viewport,
}

/// Compute the scene `layer` key for a live image handle. Fail-closed: panics
/// rather than aliasing if the generation exceeds [`LAYER_MAX_GENERATION`]
/// (stores retire slots long before that; see above).
#[must_use]
pub fn media_layer(id: ImageId) -> u64 {
    encode_layer(
        LAYER_TAG_MEDIA,
        id.raw(),
        "media image slot generation overflowed layer capacity",
    )
}

/// Compute the scene `layer` key for a viewport handle. Same fail-closed
/// capacity rule as [`media_layer`].
#[must_use]
pub fn viewport_layer(id: ViewportId) -> u64 {
    encode_layer(
        LAYER_TAG_VIEWPORT,
        id.raw(),
        "viewport slot generation overflowed layer capacity",
    )
}

fn encode_layer(tag: u64, raw: RawId, overflow_msg: &str) -> u64 {
    assert!(
        raw.generation <= LAYER_MAX_GENERATION,
        "{overflow_msg}: {} > {:?}",
        raw.generation,
        LAYER_MAX_GENERATION
    );
    tag | ((raw.generation as u64) << 32) | raw.index as u64
}

/// Split a layer key back into its namespace and handle coordinates. Returns
/// `None` for unknown namespace tags (`00`, `11`). Coordinates alone do not
/// prove liveness — the owning store must still validate the generation.
#[must_use]
pub fn decode_layer(layer: u64) -> Option<(LayerKind, RawId)> {
    let kind = match layer & LAYER_TAG_MASK {
        LAYER_TAG_MEDIA => LayerKind::Media,
        LAYER_TAG_VIEWPORT => LayerKind::Viewport,
        _ => return None,
    };
    let generation = ((layer >> 32) as u32) & LAYER_MAX_GENERATION;
    let index = layer as u32;
    Some((kind, RawId { index, generation }))
}

/// One paint operation in painter order (position in the commit sequence).
#[derive(Clone, Debug, PartialEq)]
pub enum DisplayItem {
    /// Filled rounded rect with optional border and drop shadow.
    Rect {
        /// Bounds in logical units.
        bounds: Rect,
        /// Uniform corner radius.
        radius: f32,
        /// Fill color.
        fill: Oklch,
        /// Border width (0 = no border).
        border_width: f32,
        /// Border color.
        border_color: Oklch,
        /// `(offset_x, offset_y, blur, color)`. `None` = no shadow.
        shadow: Option<(f32, f32, f32, Oklch)>,
    },
    /// Axis-aligned clip for subsequent siblings until matching `PopClip`.
    PushClip {
        /// Clip bounds.
        bounds: Rect,
        /// Uniform corner radius.
        radius: f32,
    },
    /// End the innermost clip.
    PopClip,
    /// Group opacity multiplier for subsequent siblings until `PopOpacity`.
    PushOpacity(f32),
    /// End the innermost opacity group.
    PopOpacity,
    /// In-app backdrop blur of content behind this rect.
    BackdropBlur {
        /// Area to blur.
        bounds: Rect,
        /// Blur radius (physical px at scale 1).
        radius: f32,
    },
    /// Text run. Shaping/lookup is owned by the text provider; the renderer
    /// resolves the handle. Layout owns glyph positioning.
    Text {
        /// Top-left origin in logical units.
        at: [f32; 2],
        /// Opaque run key resolved by the text provider.
        run: TextRun,
        /// Text color.
        color: Oklch,
        /// Additional opacity multiplier.
        opacity: f32,
    },
    /// External texture (image/video/viewport frame) composited in order.
    /// Produced by the media/viewport owner; the renderer only samples the
    /// provided view.
    External {
        /// Bounds in logical units.
        bounds: Rect,
        /// Opaque layer key namespaced by the media owner.
        layer: u64,
        /// Opacity multiplier.
        opacity: f32,
        /// Uniform corner radius of the sampling clip.
        rounded_clip: f32,
    },
}

impl DisplayItem {
    /// Conservative logical bounds of the item INCLUDING effect footprints
    /// (shadow blur/offset, backdrop sampling padding). Used for damage.
    /// Returns `None` for stack operations (`Pop*`) which cover nothing.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        match self {
            DisplayItem::Rect { bounds, shadow, .. } => {
                let mut r = *bounds;
                if let Some((ox, oy, blur, _)) = shadow {
                    // Shadow footprint: offset rect expanded by ~3 sigma.
                    let pad = 3.0 * blur.max(0.0);
                    r = Rect::from_edges(
                        (bounds.x + ox.min(0.0) - pad).min(bounds.x - pad),
                        (bounds.y + oy.min(0.0) - pad).min(bounds.y - pad),
                        (bounds.right() + ox.max(0.0) + pad).max(bounds.right() + pad),
                        (bounds.bottom() + oy.max(0.0) + pad).max(bounds.bottom() + pad),
                    );
                }
                Some(r)
            }
            DisplayItem::PushClip { bounds, .. }
            | DisplayItem::BackdropBlur { bounds, .. }
            | DisplayItem::External { bounds, .. } => Some(*bounds),
            DisplayItem::Text { at, .. } => {
                // Zero-size anchor; real extents come from the text provider.
                Some(Rect::from_origin_size(
                    crate::geometry::Point::new(at[0], at[1]),
                    crate::geometry::Size::ZERO,
                ))
            }
            DisplayItem::PushOpacity(_) | DisplayItem::PopClip | DisplayItem::PopOpacity => None,
        }
    }
}

/// Opaque handle to a shaped text run, minted by the text provider and
/// resolved by the renderer. `revision` bumps on every reshape so stale
/// raster caches invalidate.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextRun {
    /// Run identity.
    pub id: u64,
    /// Shape revision.
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    fn red() -> Oklch {
        Oklch::from_hex("#ff0000").unwrap()
    }

    #[test]
    fn shadow_expands_bounds() {
        let item = DisplayItem::Rect {
            bounds: Rect::from_edges(0.0, 0.0, 10.0, 10.0),
            radius: 0.0,
            fill: red(),
            border_width: 0.0,
            border_color: red(),
            shadow: Some((2.0, 4.0, 3.0, red())),
        };
        let b = item.bounds().unwrap();
        assert!(b.x < 0.0 && b.y < 0.0);
        assert!(b.right() > 10.0 && b.bottom() > 10.0);
    }

    #[test]
    fn stack_ops_cover_nothing() {
        assert_eq!(DisplayItem::PopClip.bounds(), None);
        assert_eq!(DisplayItem::PushOpacity(0.5).bounds(), None);
        assert_eq!(DisplayItem::PopOpacity.bounds(), None);
    }

    #[test]
    fn layer_keys_carry_full_generation_and_namespace() {
        use crate::id::{ImageId, ViewportId};
        // Generation above the old 24-bit mask: must survive, not truncate.
        let raw = RawId {
            index: 0xDEAD_BEEF,
            generation: 0x00FF_FFFF,
        };
        let layer = media_layer(ImageId::from_raw(raw));
        assert_eq!(layer & LAYER_TAG_MASK, LAYER_TAG_MEDIA);
        let (kind, back) = decode_layer(layer).expect("known tag decodes");
        assert_eq!(kind, LayerKind::Media);
        assert_eq!(back, raw);
        // Namespaces are disjoint for identical coordinates.
        let vlayer = viewport_layer(ViewportId::from_raw(raw));
        assert_ne!(layer, vlayer);
        let (vkind, vback) = decode_layer(vlayer).expect("known tag decodes");
        assert_eq!(vkind, LayerKind::Viewport);
        assert_eq!(vback, raw);
        // Unknown tags fail closed.
        assert_eq!(decode_layer(0x0000_0000_0000_0001), None);
        assert_eq!(decode_layer(0xC000_0000_0000_0001), None);
        // INVALID handles never form a live key (reserved index decodes only
        // as coordinates; stores still reject it via liveness checks).
        assert!(decode_layer(media_layer(ImageId::from_raw(RawId {
            index: 0,
            generation: 0
        })))
        .is_some());
    }

    #[test]
    #[should_panic(expected = "overflowed layer capacity")]
    fn layer_generation_overflow_fails_closed() {
        use crate::id::ImageId;
        let _ = media_layer(ImageId::from_raw(RawId {
            index: 0,
            generation: LAYER_MAX_GENERATION + 1,
        }));
    }
}
