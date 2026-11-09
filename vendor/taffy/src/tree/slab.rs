use core::ops::{Index, IndexMut};

use super::NodeId;

const CHUNK: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Key(u64);

impl Key {
    fn new(index: usize, version: u32) -> Self {
        Self(u64::from(version) << 32 | index as u64)
    }

    fn index(self) -> usize {
        (self.0 & 0xffff_ffff) as usize
    }

    fn version(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

impl From<NodeId> for Key {
    #[inline]
    fn from(id: NodeId) -> Self {
        Self(id.into())
    }
}

impl From<Key> for NodeId {
    #[inline]
    fn from(key: Key) -> Self {
        NodeId::new(key.0)
    }
}

#[derive(Debug, Clone)]
struct Slot<T> {
    version: u32,
    value: Option<T>,
}

#[derive(Debug, Clone)]
pub(crate) struct Slab<T> {
    chunks: Vec<Vec<Slot<T>>>,
    slots: usize,
    free: Vec<u32>,
    len: usize,
}

impl<T> Slab<T> {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            chunks: Vec::with_capacity(capacity.div_ceil(CHUNK)),
            slots: 0,
            free: Vec::new(),
            len: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn insert(&mut self, value: T) -> Key {
        self.len += 1;
        if let Some(index) = self.free.pop() {
            let slot = self.slot_mut(index as usize);
            slot.value = Some(value);
            return Key::new(index as usize, slot.version);
        }
        let index = self.slots;
        if index % CHUNK == 0 {
            self.chunks.push(Vec::with_capacity(CHUNK));
        }
        self.chunks[index / CHUNK].push(Slot {
            version: 1,
            value: Some(value),
        });
        self.slots += 1;
        Key::new(index, 1)
    }

    fn slot(&self, index: usize) -> Option<&Slot<T>> {
        self.chunks.get(index / CHUNK)?.get(index % CHUNK)
    }

    fn slot_mut(&mut self, index: usize) -> &mut Slot<T> {
        &mut self.chunks[index / CHUNK][index % CHUNK]
    }

    pub(crate) fn get(&self, key: Key) -> Option<&T> {
        let slot = self.slot(key.index())?;
        if slot.version == key.version() {
            slot.value.as_ref()
        } else {
            None
        }
    }

    pub(crate) fn get_mut(&mut self, key: Key) -> Option<&mut T> {
        let slot = self
            .chunks
            .get_mut(key.index() / CHUNK)?
            .get_mut(key.index() % CHUNK)?;
        if slot.version == key.version() {
            slot.value.as_mut()
        } else {
            None
        }
    }

    pub(crate) fn remove(&mut self, key: Key) -> Option<T> {
        self.get(key)?;
        let slot = self.slot_mut(key.index());
        let value = slot.value.take();
        slot.version = slot.version.wrapping_add(1).max(1);
        self.free.push(key.index() as u32);
        self.len -= 1;
        value
    }

    pub(crate) fn clear(&mut self) {
        let free: Vec<u32> = (0..self.slots as u32).rev().collect();
        for chunk in &mut self.chunks {
            for slot in chunk.iter_mut() {
                slot.value = None;
                slot.version = slot.version.wrapping_add(1).max(1);
            }
        }
        self.free = free;
        self.len = 0;
    }
}

impl<T> Index<Key> for Slab<T> {
    type Output = T;

    #[inline]
    fn index(&self, key: Key) -> &T {
        self.get(key).expect("invalid node key")
    }
}

impl<T> IndexMut<Key> for Slab<T> {
    #[inline]
    fn index_mut(&mut self, key: Key) -> &mut T {
        self.get_mut(key).expect("invalid node key")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Secondary<T> {
    entries: Vec<Option<(u32, T)>>,
}

impl<T> Secondary<T> {
    pub(crate) fn with_capacity(_capacity: usize) -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub(crate) fn insert(&mut self, key: Key, value: T) -> Option<T> {
        let index = key.index();
        if self.entries.len() <= index {
            let target = index.next_multiple_of(CHUNK) + CHUNK;
            self.entries.reserve_exact(target - self.entries.len());
            self.entries.resize_with(index + 1, || None);
        }
        let previous = self.entries[index].replace((key.version(), value));
        previous.and_then(|(version, old)| (version == key.version()).then_some(old))
    }

    pub(crate) fn get(&self, key: Key) -> Option<&T> {
        match self.entries.get(key.index())? {
            Some((version, value)) if *version == key.version() => Some(value),
            _ => None,
        }
    }

    pub(crate) fn get_mut(&mut self, key: Key) -> Option<&mut T> {
        match self.entries.get_mut(key.index())? {
            Some((version, value)) if *version == key.version() => Some(value),
            _ => None,
        }
    }

    pub(crate) fn remove(&mut self, key: Key) -> Option<T> {
        let entry = self.entries.get_mut(key.index())?;
        match entry {
            Some((version, _)) if *version == key.version() => entry.take().map(|(_, value)| value),
            _ => None,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}
