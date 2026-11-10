use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

use crate::{NodeId, Srgba8};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SrgbaFillPatch {
    pub node: NodeId,
    pub fill: Srgba8,
}

#[derive(Default)]
struct Buffers {
    free: Vec<Vec<SrgbaFillPatch>>,
    capacity: usize,
}

#[derive(Default)]
pub struct SrgbaFillBatch {
    patches: Vec<SrgbaFillPatch>,
    buffers: Arc<Mutex<Buffers>>,
}

impl SrgbaFillBatch {
    pub fn sibling(&self) -> Self {
        let mut buffers = self
            .buffers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        buffers.capacity = buffers.capacity.max(self.patches.capacity());
        let patches = buffers
            .free
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(buffers.capacity));
        Self {
            patches,
            buffers: Arc::clone(&self.buffers),
        }
    }
}

impl Clone for SrgbaFillBatch {
    fn clone(&self) -> Self {
        let mut copy = self.sibling();
        copy.extend_from_slice(&self.patches);
        copy
    }
}

impl PartialEq for SrgbaFillBatch {
    fn eq(&self, other: &Self) -> bool {
        self.patches == other.patches
    }
}

impl fmt::Debug for SrgbaFillBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SrgbaFillBatch")
            .field("patches", &self.patches.len())
            .finish()
    }
}

impl Deref for SrgbaFillBatch {
    type Target = Vec<SrgbaFillPatch>;

    fn deref(&self) -> &Self::Target {
        &self.patches
    }
}

impl DerefMut for SrgbaFillBatch {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.patches
    }
}

impl Drop for SrgbaFillBatch {
    fn drop(&mut self) {
        let patches = std::mem::take(&mut self.patches);
        if patches.capacity() == 0 {
            return;
        }
        let mut buffers = self
            .buffers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if buffers.free.len() < 2 {
            let mut patches = patches;
            patches.clear();
            buffers.capacity = buffers.capacity.max(patches.capacity());
            buffers.free.push(patches);
        }
    }
}
