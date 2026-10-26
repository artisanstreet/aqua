//! Stable generational IDs. A copyable handle never silently redirects to a
//! different occupant after its slot is reused: the generation mismatches and
//! every lookup fails (returns `None` / `Err`).

use std::fmt;
use std::marker::PhantomData;

/// Untyped generational handle: index into a dense slot table plus the
/// generation that owned the slot when the handle was issued.
///
/// Generations start at **0** for freshly inserted occupants and bump on every
/// removal, matching `aqua-render`'s retained scene (agreed contract). A stale
/// handle fails on lookup because its generation no longer matches.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawId {
    /// Slot index.
    pub index: u32,
    /// Generation of the occupant.
    pub generation: u32,
}

impl RawId {
    /// A handle that compares unequal to every live handle. The index
    /// `u32::MAX` is never issued, so this is inert even though its
    /// generation is 0.
    pub const INVALID: RawId = RawId {
        index: u32::MAX,
        generation: u32::MAX,
    };

    #[must_use]
    pub fn is_valid(self) -> bool {
        // Generations start at 0, so only the reserved index marks INVALID.
        self.index != u32::MAX
    }
}

impl fmt::Debug for RawId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RawId({}:{})", self.index, self.generation)
    }
}

/// Typed wrapper so `NodeId` and `ViewportId` cannot be mixed up.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id<T> {
    raw: RawId,
    _kind: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    #[must_use]
    pub const fn from_raw(raw: RawId) -> Self {
        Self {
            raw,
            _kind: PhantomData,
        }
    }

    #[must_use]
    pub const fn raw(self) -> RawId {
        self.raw
    }

    #[must_use]
    pub fn is_valid(self) -> bool {
        self.raw.is_valid()
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}({}:{})",
            std::any::type_name::<T>()
                .rsplit("::")
                .next()
                .unwrap_or("Id"),
            self.raw.index,
            self.raw.generation
        )
    }
}

// No manual Send/Sync impls: `PhantomData<fn() -> T>` already yields the
// correct auto traits (Send + Sync regardless of T).

/// Marker types giving each ID family a distinct Rust type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeTag;
/// Marker type for UI tree node identities.
pub type NodeId = Id<NodeTag>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ViewportTag;
/// Marker type for custom GPU viewport handles (`Viewport3D`, video, etc.).
pub type ViewportId = Id<ViewportTag>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageTag;
/// Marker type for raster/vector image assets referenced by scene data.
pub type ImageId = Id<ImageTag>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_handle_never_valid() {
        assert!(!RawId::INVALID.is_valid());
        assert!(!NodeId::from_raw(RawId::INVALID).is_valid());
    }

    #[test]
    fn first_generation_handles_are_valid() {
        // Fresh inserts carry generation 0 and MUST be valid: the reserved
        // index (u32::MAX), not the generation, marks INVALID.
        let first = RawId {
            index: 0,
            generation: 0,
        };
        assert!(first.is_valid());
        assert!(NodeId::from_raw(first).is_valid());
    }

    #[test]
    fn typed_ids_do_not_compare_across_families() {
        // Compile-time property: NodeId != ViewportId is a type error.
        // Runtime spot check that equal raw bits stay equal within a family.
        let raw = RawId {
            index: 3,
            generation: 7,
        };
        assert_eq!(
            NodeId::from_raw(raw),
            NodeId::from_raw(RawId {
                index: 3,
                generation: 7
            })
        );
        assert_ne!(
            NodeId::from_raw(raw),
            NodeId::from_raw(RawId {
                index: 3,
                generation: 8
            })
        );
    }
}
