//! Generational arena. Handles stay stable for the arena's lifetime: removal
//! clears the slot and bumps its generation, so stale handles fail instead of
//! aliasing a new occupant. Freed slots are reused by later inserts.
//!
//! Slots are not compacted on removal (bounded by the high-water mark); use
//! [`Arena::retain`] or rebuild when high churn leaves many holes.

use crate::id::RawId;

/// Error for stale or invalid handle access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArenaError {
    /// Slot index was never issued or is out of range.
    OutOfRange,
    /// Slot was removed and possibly reused; generations differ.
    Stale { expected: u32, found: u32 },
}

#[derive(Debug)]
struct Entry<T> {
    generation: u32,
    value: Option<T>,
    /// Set when the generation counter wraps: the slot is permanently retired
    /// and never reused, so no new handle can ever alias an outstanding one.
    retired: bool,
}

/// Bump a generation counter. Returns `false` when the counter wraps, in
/// which case the caller must retire the slot instead of reusing it.
fn bump_generation(current: u32) -> (u32, bool) {
    let (next, overflowed) = current.overflowing_add(1);
    (next, !overflowed)
}

/// Storage with stable generational handles.
#[derive(Debug)]
pub struct Arena<T> {
    entries: Vec<Entry<T>>,
    free: Vec<u32>,
    live: usize,
    /// Slots permanently retired by generation wrap (never reused).
    retired: usize,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Arena<T> {
    /// Create an empty arena.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            free: Vec::new(),
            live: 0,
            retired: 0,
        }
    }

    /// Create an empty arena with preallocated slot capacity.
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
            free: Vec::new(),
            live: 0,
            retired: 0,
        }
    }

    /// Number of live occupants.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    /// Whether no occupants are live.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Slot high-water mark (live plus vacant reusable slots).
    #[must_use]
    pub fn capacity_slots(&self) -> usize {
        self.entries.len()
    }

    /// Insert a value, returning a handle valid until removed.
    pub fn insert(&mut self, value: T) -> RawId {
        self.live += 1;
        if let Some(index) = self.free.pop() {
            let entry = &mut self.entries[index as usize];
            debug_assert!(entry.value.is_none());
            debug_assert!(!entry.retired);
            entry.value = Some(value);
            return RawId {
                index,
                generation: entry.generation,
            };
        }
        let index = self.entries.len() as u32;
        // Generations start at 0 (agreed with aqua-render's scene store).
        // `INVALID` uses index u32::MAX, which is never issued.
        self.entries.push(Entry {
            generation: 0,
            value: Some(value),
            retired: false,
        });
        RawId {
            index,
            generation: 0,
        }
    }

    /// Remove the occupant behind `id`, returning it.
    pub fn remove(&mut self, id: RawId) -> Result<T, ArenaError> {
        let entry = self
            .entries
            .get_mut(id.index as usize)
            .ok_or(ArenaError::OutOfRange)?;
        if entry.generation != id.generation || entry.value.is_none() {
            let found = entry.generation;
            return Err(if id.index as usize >= self.entries.len() {
                ArenaError::OutOfRange
            } else {
                ArenaError::Stale {
                    expected: id.generation,
                    found,
                }
            });
        }
        // Bump generation so outstanding handles go stale. If the counter
        // wraps, the slot is retired permanently instead of reused: no new
        // handle may ever alias an outstanding one.
        let (next, ok) = bump_generation(entry.generation);
        if ok {
            entry.generation = next;
            self.free.push(id.index);
        } else {
            entry.retired = true;
            self.retired += 1;
        }
        self.live -= 1;
        Ok(entry.value.take().expect("checked above"))
    }

    /// Whether `id` currently addresses a live occupant.
    #[must_use]
    pub fn contains(&self, id: RawId) -> bool {
        self.get(id).is_some()
    }

    /// Shared access; `None` for stale or out-of-range handles.
    #[must_use]
    pub fn get(&self, id: RawId) -> Option<&T> {
        let entry = self.entries.get(id.index as usize)?;
        if entry.generation == id.generation {
            entry.value.as_ref()
        } else {
            None
        }
    }

    /// Exclusive access; `None` for stale or out-of-range handles.
    pub fn get_mut(&mut self, id: RawId) -> Option<&mut T> {
        let entry = self.entries.get_mut(id.index as usize)?;
        if entry.generation == id.generation {
            entry.value.as_mut()
        } else {
            None
        }
    }

    /// Iterate live `(handle, value)` pairs in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (RawId, &T)> {
        self.entries.iter().enumerate().filter_map(|(i, e)| {
            e.value.as_ref().map(|v| {
                (
                    RawId {
                        index: i as u32,
                        generation: e.generation,
                    },
                    v,
                )
            })
        })
    }

    /// Iterate live `(handle, value)` pairs mutably in slot order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (RawId, &mut T)> {
        self.entries.iter_mut().enumerate().filter_map(|(i, e)| {
            let generation = e.generation;
            e.value.as_mut().map(|v| {
                (
                    RawId {
                        index: i as u32,
                        generation,
                    },
                    v,
                )
            })
        })
    }

    /// Remove all occupants, invalidating every outstanding handle.
    /// Slots whose generation wraps are retired, not reused (see `remove`).
    pub fn clear(&mut self) {
        self.free.clear();
        for (i, entry) in self.entries.iter_mut().enumerate() {
            entry.value = None;
            let (next, ok) = bump_generation(entry.generation);
            if ok {
                entry.generation = next;
                self.free.push(i as u32);
            } else {
                entry.retired = true;
                self.retired += 1;
            }
        }
        self.live = 0;
    }

    /// Slots permanently retired by generation wrap. Informational: proves
    /// the no-alias rule holds without reusing exhausted slots.
    #[must_use]
    pub fn retired_slots(&self) -> usize {
        self.retired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove() {
        let mut arena = Arena::new();
        let a = arena.insert("hello");
        let b = arena.insert("world");
        assert_eq!(arena.len(), 2);
        assert_eq!(arena.get(a), Some(&"hello"));
        assert_eq!(arena.get(b), Some(&"world"));
        assert_eq!(arena.remove(a), Ok("hello"));
        assert_eq!(arena.get(a), None);
        assert_eq!(arena.get(b), Some(&"world"));
        assert_eq!(arena.len(), 1);
    }

    #[test]
    fn removed_slot_reuse_does_not_alias() {
        let mut arena = Arena::new();
        let a = arena.insert(1);
        assert!(arena.remove(a).is_ok());
        let c = arena.insert(2);
        // Same slot may be reused, but the generation must differ.
        assert_ne!(a, c);
        assert_eq!(arena.get(a), None);
        assert_eq!(arena.get(c), Some(&2));
        // Double remove of the stale handle fails.
        assert!(matches!(arena.remove(a), Err(ArenaError::Stale { .. })));
    }

    #[test]
    fn out_of_range_is_distinct_from_stale() {
        let arena: Arena<u8> = Arena::new();
        let bogus = RawId {
            index: 99,
            generation: 1,
        };
        assert_eq!(arena.get(bogus), None);
        assert!(matches!(
            Arena::<u8>::new().remove(bogus),
            Err(ArenaError::OutOfRange)
        ));
    }

    #[test]
    fn invalid_handle_never_resolves() {
        let mut arena = Arena::new();
        arena.insert(10);
        assert_eq!(arena.get(RawId::INVALID), None);
        assert!(arena.remove(RawId::INVALID).is_err());
    }

    #[test]
    fn clear_invalidates_everything() {
        let mut arena = Arena::new();
        let a = arena.insert(1);
        let b = arena.insert(2);
        arena.clear();
        assert!(arena.is_empty());
        assert_eq!(arena.get(a), None);
        assert_eq!(arena.get(b), None);
        // Arena stays usable afterwards.
        let c = arena.insert(3);
        assert_eq!(arena.get(c), Some(&3));
    }

    #[test]
    fn generation_bump_never_aliases_on_wrap() {
        // Unit-test the wrap path directly: 2^32 removes are infeasible.
        let (next, ok) = bump_generation(41);
        assert_eq!((next, ok), (42, true));
        let (_, ok) = bump_generation(u32::MAX);
        assert!(!ok, "wrapped counter must force slot retirement");
    }

    #[test]
    fn iter_covers_live_only() {
        let mut arena = Arena::new();
        let a = arena.insert(1);
        arena.insert(2);
        arena.insert(3);
        arena.remove(a).unwrap();
        let values: Vec<i32> = arena.iter().map(|(_, &v)| v).collect();
        assert_eq!(values, vec![2, 3]);
    }
}
