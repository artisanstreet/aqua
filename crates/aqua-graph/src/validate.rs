use std::collections::HashMap;

use crate::analysis::{align_up, first_writers, lifetimes, transient_of};
use crate::plan::{
    Barrier, FramePlan, LoadOp, PassKind, QuadInstance, ResourceState, ShaderId, ShadowInstance,
    TextureRef, TransientId,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    UnbalancedPop,
    UnclosedScope,
    UnknownTexture {
        pass: usize,
    },
    ReadBeforeWrite {
        pass: usize,
    },
    SurfaceRead {
        pass: usize,
    },
    PersistentWrite {
        pass: usize,
    },
    ReadWriteHazard {
        pass: usize,
    },
    ShaderMismatch {
        pass: usize,
    },
    BadBindings {
        pass: usize,
    },
    NonFiniteValue {
        pass: usize,
    },
    LoadOfUndefined {
        pass: usize,
    },
    BarrierMismatch {
        pass: usize,
    },
    MissingBarrier {
        pass: usize,
    },
    MissingAliasBarrier {
        texture: TransientId,
    },
    SurfaceNotPresented,
    TextureTooLarge {
        texture: TransientId,
    },
    HeapTooSmall,
    HeapMisaligned {
        texture: TransientId,
    },
    AliasOverlap {
        first: TransientId,
        second: TransientId,
    },
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::UnbalancedPop => write!(f, "pop without matching push"),
            PlanError::UnclosedScope => write!(f, "push without matching pop"),
            PlanError::UnknownTexture { pass } => write!(f, "pass {pass} names an unknown texture"),
            PlanError::ReadBeforeWrite { pass } => {
                write!(f, "pass {pass} reads a texture nothing wrote yet")
            }
            PlanError::SurfaceRead { pass } => {
                write!(f, "pass {pass} reads the surface without being a copy")
            }
            PlanError::PersistentWrite { pass } => {
                write!(f, "pass {pass} writes a persistent texture")
            }
            PlanError::ReadWriteHazard { pass } => {
                write!(f, "pass {pass} reads and writes the same texture")
            }
            PlanError::ShaderMismatch { pass } => {
                write!(f, "pass {pass} uses a shader its kind cannot run")
            }
            PlanError::BadBindings { pass } => {
                write!(f, "pass {pass} declares the wrong reads or writes")
            }
            PlanError::NonFiniteValue { pass } => {
                write!(f, "pass {pass} carries a non-finite or out-of-range value")
            }
            PlanError::LoadOfUndefined { pass } => {
                write!(
                    f,
                    "pass {pass} loads a transient whose contents are undefined"
                )
            }
            PlanError::BarrierMismatch { pass } => {
                write!(
                    f,
                    "pass {pass} transitions a resource from a state it is not in"
                )
            }
            PlanError::MissingBarrier { pass } => {
                write!(
                    f,
                    "pass {pass} uses a resource in a state no barrier established"
                )
            }
            PlanError::MissingAliasBarrier { texture } => write!(
                f,
                "transient {} reuses heap memory without an aliasing barrier",
                texture.0
            ),
            PlanError::SurfaceNotPresented => {
                write!(f, "surface does not end in the present state")
            }
            PlanError::TextureTooLarge { texture } => {
                write!(
                    f,
                    "transient {} exceeds the maximum texture extent",
                    texture.0
                )
            }
            PlanError::HeapTooSmall => write!(f, "heap size does not cover every transient"),
            PlanError::HeapMisaligned { texture } => {
                write!(
                    f,
                    "transient {} is not aligned to the heap alignment",
                    texture.0
                )
            }
            PlanError::AliasOverlap { first, second } => write!(
                f,
                "transients {} and {} are live together on overlapping heap ranges",
                first.0, second.0
            ),
        }
    }
}

impl std::error::Error for PlanError {}

fn finite_rect(rect: aqua_core::geometry::Rect) -> bool {
    [rect.x, rect.y, rect.w, rect.h]
        .iter()
        .all(|v| v.is_finite())
        && rect.w >= 0.0
        && rect.h >= 0.0
}

fn finite_quad(instance: &QuadInstance) -> bool {
    finite_rect(instance.bounds)
        && instance.radius.is_finite()
        && instance.radius >= 0.0
        && instance.border_width.is_finite()
        && instance.border_width >= 0.0
        && instance.opacity.is_finite()
        && (0.0..=1.0).contains(&instance.opacity)
        && instance.clip.is_none_or(finite_rect)
}

fn finite_shadow(instance: &ShadowInstance) -> bool {
    finite_rect(instance.bounds)
        && [
            instance.radius,
            instance.sigma,
            instance.spread,
            instance.offset[0],
            instance.offset[1],
        ]
        .iter()
        .all(|v| v.is_finite())
        && instance.radius >= 0.0
        && instance.sigma >= 0.0
        && instance.opacity.is_finite()
        && (0.0..=1.0).contains(&instance.opacity)
        && instance.clip.is_none_or(finite_rect)
}

fn sorted_unique(mut refs: Vec<TextureRef>) -> Vec<TextureRef> {
    let key = |reference: &TextureRef| match reference {
        TextureRef::Surface => (0u8, 0u32),
        TextureRef::Transient(id) => (1, id.0),
        TextureRef::Persistent(handle) => (2, handle.0),
    };
    refs.sort_by_key(key);
    refs.dedup();
    refs
}

fn check_kind(plan: &FramePlan, index: usize) -> Result<(), PlanError> {
    let pass = &plan.passes[index];
    let bad_bindings = PlanError::BadBindings { pass: index };
    let non_finite = PlanError::NonFiniteValue { pass: index };
    let single_target = pass.writes.len() == 1;
    match &pass.kind {
        PassKind::Instanced { shader, instances } => {
            if *shader != ShaderId::Quad {
                return Err(PlanError::ShaderMismatch { pass: index });
            }
            if !pass.reads.is_empty() || !single_target {
                return Err(bad_bindings);
            }
            instances
                .iter()
                .all(finite_quad)
                .then_some(())
                .ok_or(non_finite)
        }
        PassKind::Shadows { instances } => {
            if !pass.reads.is_empty() || !single_target {
                return Err(bad_bindings);
            }
            instances
                .iter()
                .all(finite_shadow)
                .then_some(())
                .ok_or(non_finite)
        }
        PassKind::Glyphs { atlas, instances } => {
            if pass.reads.as_slice() != [TextureRef::Persistent(*atlas)] || !single_target {
                return Err(bad_bindings);
            }
            instances
                .iter()
                .all(|glyph| {
                    finite_rect(glyph.bounds)
                        && finite_rect(glyph.texels)
                        && (0.0..=1.0).contains(&glyph.opacity)
                        && glyph.clip.is_none_or(finite_rect)
                })
                .then_some(())
                .ok_or(non_finite)
        }
        PassKind::Images { instances } => {
            if !single_target {
                return Err(bad_bindings);
            }
            let declared = sorted_unique(instances.iter().map(|image| image.source).collect());
            if declared != sorted_unique(pass.reads.clone()) {
                return Err(bad_bindings);
            }
            instances
                .iter()
                .all(|image| {
                    finite_rect(image.bounds)
                        && finite_rect(image.texels)
                        && image.radius.is_finite()
                        && image.radius >= 0.0
                        && (0.0..=1.0).contains(&image.opacity)
                        && image.clip.is_none_or(finite_rect)
                })
                .then_some(())
                .ok_or(non_finite)
        }
        PassKind::Fullscreen { shader, opacity } => {
            if *shader != ShaderId::LayerOver {
                return Err(PlanError::ShaderMismatch { pass: index });
            }
            if pass.reads.len() != 1 || !single_target {
                return Err(bad_bindings);
            }
            (opacity.is_finite() && (0.0..=1.0).contains(opacity))
                .then_some(())
                .ok_or(non_finite)
        }
        PassKind::Filter { shader, params } => {
            if !matches!(
                shader,
                ShaderId::BlurDown | ShaderId::BlurUp | ShaderId::ColorMatrix | ShaderId::Custom(_)
            ) {
                return Err(PlanError::ShaderMismatch { pass: index });
            }
            if pass.reads.len() != 1 || !single_target {
                return Err(bad_bindings);
            }
            (finite_rect(params.region) && params.values.iter().all(|v| v.is_finite()))
                .then_some(())
                .ok_or(non_finite)
        }
        PassKind::Compute { .. } | PassKind::Custom(_) => {
            (!pass.writes.is_empty()).then_some(()).ok_or(bad_bindings)
        }
        PassKind::Copy { size, .. } => {
            let source_ok = matches!(
                pass.reads.as_slice(),
                [TextureRef::Surface] | [TextureRef::Transient(_)]
            );
            let dest_ok = matches!(pass.writes.as_slice(), [TextureRef::Transient(_)]);
            (source_ok && dest_ok && !size.is_empty())
                .then_some(())
                .ok_or(bad_bindings)
        }
    }
}

fn check_states(plan: &FramePlan) -> Result<(), PlanError> {
    let mut state: HashMap<TextureRef, ResourceState> = HashMap::new();
    state.insert(TextureRef::Surface, ResourceState::Present);
    let current = |state: &HashMap<TextureRef, ResourceState>, reference: TextureRef| {
        state.get(&reference).copied().unwrap_or(match reference {
            TextureRef::Persistent(_) => ResourceState::ShaderRead,
            _ => ResourceState::Undefined,
        })
    };
    for (index, pass) in plan.passes.iter().enumerate() {
        for barrier in &pass.barriers {
            match barrier {
                Barrier::Transition { resource, from, to } => {
                    if current(&state, *resource) != *from {
                        return Err(PlanError::BarrierMismatch { pass: index });
                    }
                    state.insert(*resource, *to);
                }
                Barrier::Alias { after, .. } => {
                    state.insert(TextureRef::Transient(*after), ResourceState::Undefined);
                }
            }
        }
        let wanted = pass
            .reads
            .iter()
            .map(|read| (*read, pass.kind.read_state()))
            .chain(
                pass.writes
                    .iter()
                    .map(|write| (*write, pass.kind.write_state())),
            );
        for (reference, required) in wanted {
            let actual = current(&state, reference);
            let satisfied = actual == required
                || (matches!(reference, TextureRef::Persistent(_))
                    && required == ResourceState::ShaderRead);
            if !satisfied {
                return Err(PlanError::MissingBarrier { pass: index });
            }
        }
    }
    for barrier in &plan.final_barriers {
        if let Barrier::Transition { resource, from, to } = barrier {
            if current(&state, *resource) != *from {
                return Err(PlanError::BarrierMismatch {
                    pass: plan.passes.len(),
                });
            }
            state.insert(*resource, *to);
        }
    }
    (current(&state, TextureRef::Surface) == ResourceState::Present)
        .then_some(())
        .ok_or(PlanError::SurfaceNotPresented)
}

pub fn validate(plan: &FramePlan) -> Result<(), PlanError> {
    let mut written: HashMap<TransientId, usize> = HashMap::new();
    let known = |reference: &TextureRef| match reference {
        TextureRef::Surface | TextureRef::Persistent(_) => true,
        TextureRef::Transient(id) => plan.texture(*id).is_some(),
    };
    let first = first_writers(&plan.passes);
    for (index, pass) in plan.passes.iter().enumerate() {
        if !pass.reads.iter().chain(&pass.writes).all(known) {
            return Err(PlanError::UnknownTexture { pass: index });
        }
        let is_copy = matches!(pass.kind, PassKind::Copy { .. });
        if pass.reads.contains(&TextureRef::Surface) && !is_copy {
            return Err(PlanError::SurfaceRead { pass: index });
        }
        if pass
            .writes
            .iter()
            .any(|write| matches!(write, TextureRef::Persistent(_)))
        {
            return Err(PlanError::PersistentWrite { pass: index });
        }
        if pass.reads.iter().any(|read| pass.writes.contains(read)) {
            return Err(PlanError::ReadWriteHazard { pass: index });
        }
        check_kind(plan, index)?;
        for read in pass.reads.iter().filter_map(transient_of) {
            if !written.contains_key(&read) {
                return Err(PlanError::ReadBeforeWrite { pass: index });
            }
        }
        for write in &pass.writes {
            if let TextureRef::Transient(id) = write {
                written.entry(*id).or_insert(index);
                let is_first = first.get(write) == Some(&index);
                if is_first && pass.load == LoadOp::Load {
                    return Err(PlanError::LoadOfUndefined { pass: index });
                }
            }
        }
    }
    let lives = lifetimes(&plan.passes);
    let used: Vec<_> = plan
        .textures
        .iter()
        .filter(|texture| lives.contains_key(&texture.id))
        .collect();
    let mut heap_end = 0;
    for texture in &used {
        heap_end = heap_end.max(texture.heap_offset + texture.bytes());
    }
    if plan.heap_bytes < heap_end {
        return Err(PlanError::HeapTooSmall);
    }
    for (a_index, a) in used.iter().enumerate() {
        if a.heap_offset != align_up(a.heap_offset, 256) {
            return Err(PlanError::HeapMisaligned { texture: a.id });
        }
        for b in &used[a_index + 1..] {
            if !lives[&a.id].overlaps(lives[&b.id]) {
                continue;
            }
            let overlap = a.heap_offset < b.heap_offset + b.bytes()
                && b.heap_offset < a.heap_offset + a.bytes();
            if overlap {
                return Err(PlanError::AliasOverlap {
                    first: a.id,
                    second: b.id,
                });
            }
        }
    }
    for a in &used {
        for b in &used {
            if a.id == b.id || lives[&b.id].last >= lives[&a.id].first {
                continue;
            }
            let overlap = a.heap_offset < b.heap_offset + b.bytes()
                && b.heap_offset < a.heap_offset + a.bytes();
            if !overlap {
                continue;
            }
            let covered = plan.passes[lives[&a.id].first]
                .barriers
                .iter()
                .any(|barrier| matches!(barrier, Barrier::Alias { after, .. } if *after == a.id));
            if !covered {
                return Err(PlanError::MissingAliasBarrier { texture: a.id });
            }
        }
    }
    check_states(plan)
}
