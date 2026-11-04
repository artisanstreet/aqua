use aqua_graph::{Barrier, FramePlan, PassKind, TextureHandle, TextureRef};
use aqua_protocol::UploadBatch;

pub const NAMESPACE_SHIFT: u32 = 24;
pub const NAMESPACE_LIMIT: u32 = 1 << (32 - NAMESPACE_SHIFT);

fn handle(namespace: u32, texture: TextureHandle) -> TextureHandle {
    TextureHandle(texture.0 | (namespace << NAMESPACE_SHIFT))
}

fn reference(namespace: u32, texture: TextureRef) -> TextureRef {
    match texture {
        TextureRef::Persistent(texture) => TextureRef::Persistent(handle(namespace, texture)),
        other => other,
    }
}

pub fn namespace_uploads(namespace: u32, batch: &mut UploadBatch) {
    if namespace == 0 {
        return;
    }
    batch
        .textures
        .iter_mut()
        .for_each(|upload| upload.handle = handle(namespace, upload.handle));
}

pub fn namespace_plan(namespace: u32, plan: &mut FramePlan) {
    if namespace == 0 {
        return;
    }
    plan.final_barriers
        .iter_mut()
        .for_each(|barrier| namespace_barrier(namespace, barrier));
    for pass in &mut plan.passes {
        pass.reads
            .iter_mut()
            .chain(pass.writes.iter_mut())
            .for_each(|texture| *texture = reference(namespace, *texture));
        pass.barriers
            .iter_mut()
            .for_each(|barrier| namespace_barrier(namespace, barrier));
        match &mut pass.kind {
            PassKind::Glyphs { atlas, .. } => *atlas = handle(namespace, *atlas),
            PassKind::Slug { store, .. } => *store = handle(namespace, *store),
            PassKind::Images { instances } => instances
                .iter_mut()
                .for_each(|image| image.source = reference(namespace, image.source)),
            _ => {}
        }
    }
}

fn namespace_barrier(namespace: u32, barrier: &mut Barrier) {
    if let Barrier::Transition { resource, .. } = barrier {
        *resource = reference(namespace, *resource);
    }
}

#[derive(Debug, Default)]
pub struct Namespaces {
    taken: Vec<bool>,
}

impl Namespaces {
    pub fn acquire(&mut self) -> Option<u32> {
        let free = self
            .taken
            .iter()
            .position(|taken| !taken)
            .unwrap_or(self.taken.len());
        match u32::try_from(free) {
            Ok(index) if index < NAMESPACE_LIMIT => {
                if index as usize == self.taken.len() {
                    self.taken.push(false);
                }
                self.taken[index as usize] = true;
                Some(index)
            }
            _ => None,
        }
    }

    pub fn release(&mut self, namespace: u32) {
        if let Some(slot) = self.taken.get_mut(namespace as usize) {
            *slot = false;
        }
    }
}
