use crate::analysis::{Lifetime, align_up, lifetime_of, transient_of};
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
    NotTransformable,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::UnbalancedPop => write!(f, "pop without matching push"),
            PlanError::UnclosedScope => write!(f, "push without matching pop"),
            PlanError::NotTransformable => write!(
                f,
                "only quads, images and slug glyphs take a linear transform"
            ),
            PlanError::UnknownTexture { pass } => write!(f, "pass {pass} names an unknown texture"),
            PlanError::ReadBeforeWrite { pass } => {
                write!(f, "pass {pass} reads a texture nothing wrote yet")
            }
            PlanError::SurfaceRead { pass } => {
                write!(
                    f,
                    "pass {pass} reads the surface without being a copy or a blur downsample"
                )
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

const MAX_PASS_INPUTS: usize = 8;
const MAX_GROUPS: u32 = 65535;

fn finite_quad(instance: &QuadInstance) -> bool {
    finite_rect(instance.bounds)
        && instance.radius.is_finite()
        && instance.radius >= 0.0
        && instance.border_width.is_finite()
        && instance.border_width >= 0.0
        && instance.opacity.is_finite()
        && (0.0..=1.0).contains(&instance.opacity)
        && instance.clip.is_none_or(finite_rect)
        && instance.linear.is_invertible()
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

fn same_textures(sources: impl Iterator<Item = TextureRef> + Clone, reads: &[TextureRef]) -> bool {
    sources.clone().all(|source| reads.contains(&source))
        && reads
            .iter()
            .all(|read| sources.clone().any(|source| source == *read))
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
            if !same_textures(instances.iter().map(|image| image.source), &pass.reads) {
                return Err(bad_bindings);
            }
            instances
                .iter()
                .all(|image| {
                    finite_rect(image.bounds)
                        && finite_rect(image.texels)
                        && image.radius.is_finite()
                        && image.radius >= 0.0
                        && image.clip_radius.is_finite()
                        && image.clip_radius >= 0.0
                        && (0.0..=1.0).contains(&image.opacity)
                        && image.clip.is_none_or(finite_rect)
                        && image.linear.is_invertible()
                })
                .then_some(())
                .ok_or(non_finite)
        }
        PassKind::Slug { store, instances } => {
            if pass.reads.as_slice() != [TextureRef::Persistent(*store)] || !single_target {
                return Err(bad_bindings);
            }
            instances
                .iter()
                .all(|glyph| {
                    glyph.origin.iter().all(|value| value.is_finite())
                        && glyph.linear.is_invertible()
                        && glyph.em_bounds.iter().all(|value| value.is_finite())
                        && (0.0..=1.0).contains(&glyph.opacity)
                        && glyph.clip.is_none_or(finite_rect)
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
            (finite_rect(params.region)
                && params.values.iter().all(|v| v.is_finite())
                && params.matrix.is_finite())
            .then_some(())
            .ok_or(non_finite)
        }
        PassKind::Compute {
            shader,
            groups,
            params,
        } => {
            if !matches!(shader, ShaderId::Custom(_)) {
                return Err(PlanError::ShaderMismatch { pass: index });
            }
            if !single_target || pass.reads.len() > MAX_PASS_INPUTS {
                return Err(bad_bindings);
            }
            (groups.iter().all(|group| (1..=MAX_GROUPS).contains(group))
                && params.iter().all(|value| value.is_finite()))
            .then_some(())
            .ok_or(non_finite)
        }
        PassKind::Custom(spec) => {
            if !single_target || pass.reads.len() > MAX_PASS_INPUTS {
                return Err(bad_bindings);
            }
            if spec.name.is_empty() || spec.entry.is_empty() {
                return Err(bad_bindings);
            }
            (finite_rect(spec.bounds) && spec.params.iter().all(|value| value.is_finite()))
                .then_some(())
                .ok_or(non_finite)
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

fn default_state(reference: TextureRef) -> ResourceState {
    match reference {
        TextureRef::Surface => ResourceState::Present,
        TextureRef::Persistent(_) => ResourceState::ShaderRead,
        TextureRef::Transient(_) => ResourceState::Undefined,
    }
}

fn barriers_of(plan: &FramePlan, pass: usize) -> &[Barrier] {
    match plan.passes.get(pass) {
        Some(pass) => &pass.barriers,
        None => &plan.final_barriers,
    }
}

fn state_before(
    plan: &FramePlan,
    pass: usize,
    barrier: usize,
    reference: TextureRef,
) -> ResourceState {
    let mut at = pass;
    let mut end = barrier;
    loop {
        for entry in barriers_of(plan, at)[..end].iter().rev() {
            match entry {
                Barrier::Transition { resource, to, .. } if *resource == reference => return *to,
                Barrier::Alias { after, .. } if reference == TextureRef::Transient(*after) => {
                    return ResourceState::Undefined;
                }
                _ => {}
            }
        }
        match at {
            0 => return default_state(reference),
            _ => {
                at -= 1;
                end = barriers_of(plan, at).len();
            }
        }
    }
}

fn check_states(plan: &FramePlan) -> Result<(), PlanError> {
    for (index, pass) in plan.passes.iter().enumerate() {
        for (position, barrier) in pass.barriers.iter().enumerate() {
            if let Barrier::Transition { resource, from, .. } = barrier {
                if state_before(plan, index, position, *resource) != *from {
                    return Err(PlanError::BarrierMismatch { pass: index });
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
            let actual = state_before(plan, index, pass.barriers.len(), reference);
            let satisfied = actual == required
                || (matches!(reference, TextureRef::Persistent(_))
                    && required == ResourceState::ShaderRead);
            if !satisfied {
                return Err(PlanError::MissingBarrier { pass: index });
            }
        }
    }
    let end = plan.passes.len();
    for (position, barrier) in plan.final_barriers.iter().enumerate() {
        if let Barrier::Transition { resource, from, .. } = barrier {
            if state_before(plan, end, position, *resource) != *from {
                return Err(PlanError::BarrierMismatch { pass: end });
            }
        }
    }
    (state_before(plan, end, plan.final_barriers.len(), TextureRef::Surface)
        == ResourceState::Present)
        .then_some(())
        .ok_or(PlanError::SurfaceNotPresented)
}

const CACHED_LIFETIMES: usize = 64;

struct Lives<'a> {
    plan: &'a FramePlan,
    cached: [Option<Lifetime>; CACHED_LIFETIMES],
}

impl<'a> Lives<'a> {
    fn new(plan: &'a FramePlan) -> Self {
        let mut cached = [None; CACHED_LIFETIMES];
        for (slot, texture) in cached.iter_mut().zip(&plan.textures) {
            *slot = lifetime_of(&plan.passes, texture.id);
        }
        Self { plan, cached }
    }

    fn of(&self, position: usize) -> Option<Lifetime> {
        match self.cached.get(position) {
            Some(known) => *known,
            None => lifetime_of(&self.plan.passes, self.plan.textures[position].id),
        }
    }
}

fn written_before(plan: &FramePlan, pass: usize, id: TransientId) -> bool {
    plan.passes[..pass]
        .iter()
        .any(|earlier| earlier.writes.contains(&TextureRef::Transient(id)))
}

pub fn validate(plan: &FramePlan) -> Result<(), PlanError> {
    let known = |reference: &TextureRef| match reference {
        TextureRef::Surface | TextureRef::Persistent(_) => true,
        TextureRef::Transient(id) => plan.texture(*id).is_some(),
    };
    for (index, pass) in plan.passes.iter().enumerate() {
        if !pass.reads.iter().chain(&pass.writes).all(known) {
            return Err(PlanError::UnknownTexture { pass: index });
        }
        let may_read_surface = matches!(
            pass.kind,
            PassKind::Copy { .. }
                | PassKind::Filter {
                    shader: ShaderId::BlurDown,
                    ..
                }
        );
        if pass.reads.contains(&TextureRef::Surface) && !may_read_surface {
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
            if !written_before(plan, index, read) {
                return Err(PlanError::ReadBeforeWrite { pass: index });
            }
        }
        for write in pass.writes.iter().filter_map(transient_of) {
            if pass.load == LoadOp::Load && !written_before(plan, index, write) {
                return Err(PlanError::LoadOfUndefined { pass: index });
            }
        }
    }
    let lives = Lives::new(plan);
    let mut heap_end = 0;
    for (position, texture) in plan.textures.iter().enumerate() {
        if lives.of(position).is_some() {
            heap_end = heap_end.max(texture.heap_offset + texture.bytes());
        }
    }
    if plan.heap_bytes < heap_end {
        return Err(PlanError::HeapTooSmall);
    }
    for (a_position, a) in plan.textures.iter().enumerate() {
        let Some(a_life) = lives.of(a_position) else {
            continue;
        };
        if a.heap_offset != align_up(a.heap_offset, 256) {
            return Err(PlanError::HeapMisaligned { texture: a.id });
        }
        for (b_position, b) in plan.textures.iter().enumerate().skip(a_position + 1) {
            let Some(b_life) = lives.of(b_position) else {
                continue;
            };
            if !a_life.overlaps(b_life) {
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
    for (a_position, a) in plan.textures.iter().enumerate() {
        let Some(a_life) = lives.of(a_position) else {
            continue;
        };
        for (b_position, b) in plan.textures.iter().enumerate() {
            let Some(b_life) = lives.of(b_position) else {
                continue;
            };
            if a.id == b.id || b_life.last >= a_life.first {
                continue;
            }
            let overlap = a.heap_offset < b.heap_offset + b.bytes()
                && b.heap_offset < a.heap_offset + a.bytes();
            if !overlap {
                continue;
            }
            let covered = plan.passes[a_life.first]
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
