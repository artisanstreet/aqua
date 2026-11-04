use std::collections::{HashMap, HashSet};

use aqua_core::Oklch;

use crate::analysis::{Lifetime, align_up, first_writers, independent, intersect, lifetimes};
use crate::plan::{
    Barrier, Extent, FramePlan, LoadOp, Pass, PassKind, ResourceState, TextureFormat, TextureRef,
    TransientId, TransientTexture,
};
use crate::validate::{PlanError, validate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanLimits {
    pub heap_alignment: u64,
    pub max_texture_extent: u32,
    pub bindless: bool,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            heap_alignment: 64 * 1024,
            max_texture_extent: 16384,
            bindless: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceDesc {
    pub extent: Extent,
    pub format: TextureFormat,
}

#[derive(Clone, Debug)]
pub struct RenderGraph {
    pub extent: Extent,
    pub clear: Oklch,
    resources: Vec<ResourceDesc>,
    passes: Vec<Pass>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlanStats {
    pub culled_passes: usize,
    pub culled_instances: usize,
    pub merged_passes: usize,
    pub aliased_bytes_saved: u64,
}

impl RenderGraph {
    pub fn new(extent: Extent, clear: Oklch) -> Self {
        Self {
            extent,
            clear,
            resources: Vec::new(),
            passes: Vec::new(),
        }
    }

    pub fn create(&mut self, extent: Extent, format: TextureFormat) -> TextureRef {
        let id = TransientId(self.resources.len() as u32);
        self.resources.push(ResourceDesc { extent, format });
        TextureRef::Transient(id)
    }

    pub fn push(&mut self, pass: Pass) {
        self.passes.push(pass);
    }

    pub fn passes(&self) -> &[Pass] {
        &self.passes
    }

    pub fn extent_of(&self, reference: TextureRef) -> Option<Extent> {
        match reference {
            TextureRef::Surface => Some(self.extent),
            TextureRef::Transient(id) => self.resources.get(id.0 as usize).map(|r| r.extent),
            TextureRef::Persistent(_) => None,
        }
    }

    pub fn plan(self, limits: PlanLimits) -> Result<FramePlan, PlanError> {
        self.plan_with_stats(limits).map(|(plan, _)| plan)
    }

    pub fn plan_with_stats(self, limits: PlanLimits) -> Result<(FramePlan, PlanStats), PlanError> {
        let mut stats = PlanStats::default();
        self.check_resources(limits)?;
        let mut passes = self.passes.clone();
        stats.culled_instances = cull_instances(&self, &mut passes);
        passes = drop_unfed(passes);
        let before = passes.len();
        passes = cull_passes(passes);
        stats.culled_passes = before - passes.len();
        let before = passes.len();
        passes = merge_passes(passes);
        stats.merged_passes = before - passes.len();
        let lives = lifetimes(&passes);
        let (textures, heap_bytes, naive) = allocate(&self, &lives, limits.heap_alignment);
        stats.aliased_bytes_saved = naive.saturating_sub(heap_bytes);
        assign_loads(&mut passes, self.clear);
        let final_barriers = derive_barriers(&mut passes, &textures, &lives);
        let plan = FramePlan {
            extent: self.extent,
            clear: self.clear,
            textures,
            passes,
            heap_bytes,
            final_barriers,
        };
        validate(&plan)?;
        Ok((plan, stats))
    }

    fn check_resources(&self, limits: PlanLimits) -> Result<(), PlanError> {
        self.resources
            .iter()
            .enumerate()
            .find(|(_, desc)| {
                desc.extent.width > limits.max_texture_extent
                    || desc.extent.height > limits.max_texture_extent
            })
            .map_or(Ok(()), |(index, _)| {
                Err(PlanError::TextureTooLarge {
                    texture: TransientId(index as u32),
                })
            })
    }
}

fn target_extent(graph: &RenderGraph, pass: &Pass) -> Extent {
    pass.writes
        .first()
        .and_then(|target| graph.extent_of(*target))
        .unwrap_or(graph.extent)
}

fn visible(
    bounds: aqua_core::geometry::Rect,
    clip: Option<aqua_core::geometry::Rect>,
    target: Extent,
) -> bool {
    let inside = intersect(bounds, target.bounds());
    match (inside, clip) {
        (None, _) => false,
        (Some(area), Some(clip)) => intersect(area, clip).is_some(),
        (Some(_), None) => true,
    }
}

fn cull_instances(graph: &RenderGraph, passes: &mut Vec<Pass>) -> usize {
    let mut culled = 0;
    for pass in passes.iter_mut() {
        let target = target_extent(graph, pass);
        let before = pass.kind.draw_count();
        match &mut pass.kind {
            PassKind::Instanced { instances, .. } => instances.retain(|quad| {
                let paints = quad.opacity > 0.0
                    && (quad.fill.alpha > 0.0
                        || quad.gradient.is_some()
                        || (quad.border_width > 0.0 && quad.border_color.alpha > 0.0));
                paints && visible(quad.bounds, quad.clip, target)
            }),
            PassKind::Shadows { instances } => instances.retain(|shadow| {
                shadow.opacity > 0.0
                    && shadow.color.alpha > 0.0
                    && visible(shadow.covered(), shadow.clip, target)
            }),
            PassKind::Glyphs { instances, .. } => instances.retain(|glyph| {
                glyph.opacity > 0.0
                    && glyph.color.alpha > 0.0
                    && visible(glyph.bounds, glyph.clip, target)
            }),
            PassKind::Images { instances } => instances
                .retain(|image| image.opacity > 0.0 && visible(image.bounds, image.clip, target)),
            PassKind::Fullscreen { .. }
            | PassKind::Filter { .. }
            | PassKind::Compute { .. }
            | PassKind::Copy { .. }
            | PassKind::Custom(_) => {}
        }
        culled += before - pass.kind.draw_count();
    }
    passes.retain(|pass| {
        let drawing = matches!(
            pass.kind,
            PassKind::Instanced { .. }
                | PassKind::Shadows { .. }
                | PassKind::Glyphs { .. }
                | PassKind::Images { .. }
        );
        !drawing || pass.kind.draw_count() > 0
    });
    culled
}

fn drop_unfed(passes: Vec<Pass>) -> Vec<Pass> {
    let mut written: HashSet<TextureRef> = HashSet::new();
    passes
        .into_iter()
        .filter(|pass| {
            let fed = pass
                .reads
                .iter()
                .all(|read| !matches!(read, TextureRef::Transient(_)) || written.contains(read));
            if fed {
                written.extend(pass.writes.iter().copied());
            }
            fed
        })
        .collect()
}

fn cull_passes(passes: Vec<Pass>) -> Vec<Pass> {
    let mut live: HashSet<TextureRef> = HashSet::from([TextureRef::Surface]);
    let mut keep = vec![false; passes.len()];
    for (index, pass) in passes.iter().enumerate().rev() {
        if pass.writes.iter().any(|write| live.contains(write)) {
            keep[index] = true;
            live.extend(pass.reads.iter().copied());
            live.extend(pass.writes.iter().copied());
        }
    }
    passes
        .into_iter()
        .zip(keep)
        .filter_map(|(pass, keep)| keep.then_some(pass))
        .collect()
}

fn same_batch(a: &Pass, b: &Pass) -> bool {
    a.writes == b.writes && a.load == b.load && a.constants == b.constants && {
        match (&a.kind, &b.kind) {
            (PassKind::Instanced { shader: x, .. }, PassKind::Instanced { shader: y, .. }) => {
                x == y
            }
            (PassKind::Shadows { .. }, PassKind::Shadows { .. }) => true,
            (PassKind::Glyphs { atlas: x, .. }, PassKind::Glyphs { atlas: y, .. }) => x == y,
            (PassKind::Images { .. }, PassKind::Images { .. }) => true,
            _ => false,
        }
    }
}

fn absorb(target: &mut Pass, source: Pass) {
    match (&mut target.kind, source.kind) {
        (
            PassKind::Instanced { instances, .. },
            PassKind::Instanced {
                instances: more, ..
            },
        ) => instances.extend(more),
        (PassKind::Shadows { instances }, PassKind::Shadows { instances: more }) => {
            instances.extend(more)
        }
        (
            PassKind::Glyphs { instances, .. },
            PassKind::Glyphs {
                instances: more, ..
            },
        ) => instances.extend(more),
        (PassKind::Images { instances }, PassKind::Images { instances: more }) => {
            instances.extend(more);
            source.reads.into_iter().for_each(|read| {
                if !target.reads.contains(&read) {
                    target.reads.push(read);
                }
            });
        }
        _ => {}
    }
}

fn merge_passes(passes: Vec<Pass>) -> Vec<Pass> {
    let mut result: Vec<Pass> = Vec::with_capacity(passes.len());
    for pass in passes {
        let mut destination = None;
        for (index, earlier) in result.iter().enumerate().rev() {
            if same_batch(earlier, &pass) {
                destination = Some(index);
                break;
            }
            if !independent(earlier, &pass) {
                break;
            }
        }
        match destination {
            Some(index) => absorb(&mut result[index], pass),
            None => result.push(pass),
        }
    }
    result
}

fn allocate(
    graph: &RenderGraph,
    lives: &HashMap<TransientId, Lifetime>,
    alignment: u64,
) -> (Vec<TransientTexture>, u64, u64) {
    let mut order: Vec<(TransientId, Lifetime)> = lives.iter().map(|(id, l)| (*id, *l)).collect();
    order.sort_by_key(|(id, life)| (life.first, id.0));
    let mut placed: Vec<(Lifetime, u64, u64)> = Vec::new();
    let mut textures = Vec::new();
    let mut naive = 0;
    for (id, life) in order {
        let desc = graph.resources[id.0 as usize];
        let size = align_up(
            desc.extent.texels() * desc.format.bytes_per_texel(),
            alignment,
        );
        naive += size;
        let mut taken: Vec<(u64, u64)> = placed
            .iter()
            .filter(|(other, _, _)| other.overlaps(life))
            .map(|(_, offset, size)| (*offset, *size))
            .collect();
        taken.sort();
        let mut candidate = 0;
        for (start, length) in taken {
            if candidate + size <= start {
                break;
            }
            candidate = candidate.max(align_up(start + length, alignment));
        }
        placed.push((life, candidate, size));
        textures.push(TransientTexture {
            id,
            extent: desc.extent,
            format: desc.format,
            heap_offset: candidate,
        });
    }
    textures.sort_by_key(|texture| texture.id);
    let heap = placed
        .iter()
        .map(|(_, offset, size)| offset + size)
        .max()
        .unwrap_or(0);
    (textures, heap, naive)
}

fn assign_loads(passes: &mut [Pass], clear: Oklch) {
    let first = first_writers(passes);
    for (index, pass) in passes.iter_mut().enumerate() {
        let Some(target) = pass.writes.first().copied() else {
            continue;
        };
        if first.get(&target) != Some(&index) {
            continue;
        }
        pass.load = match (target, &pass.kind) {
            (TextureRef::Surface, _) => LoadOp::Clear(clear),
            (_, PassKind::Copy { .. } | PassKind::Filter { .. } | PassKind::Compute { .. }) => {
                LoadOp::DontCare
            }
            _ => LoadOp::Clear(Oklch::TRANSPARENT),
        };
    }
}

fn derive_barriers(
    passes: &mut [Pass],
    textures: &[TransientTexture],
    lives: &HashMap<TransientId, Lifetime>,
) -> Vec<Barrier> {
    let mut state: HashMap<TextureRef, ResourceState> = HashMap::new();
    state.insert(TextureRef::Surface, ResourceState::Present);
    let overlaps = |a: &TransientTexture, b: &TransientTexture| {
        a.heap_offset < b.heap_offset + b.bytes() && b.heap_offset < a.heap_offset + a.bytes()
    };
    for (index, pass) in passes.iter_mut().enumerate() {
        let mut barriers = Vec::new();
        for texture in textures {
            if lives.get(&texture.id).map(|l| l.first) != Some(index) {
                continue;
            }
            let predecessor = textures
                .iter()
                .filter(|other| other.id != texture.id && overlaps(texture, other))
                .filter_map(|other| lives.get(&other.id).map(|l| (other, *l)))
                .filter(|(_, life)| life.last < index)
                .max_by_key(|(_, life)| life.last);
            if let Some((other, _)) = predecessor {
                barriers.push(Barrier::Alias {
                    before: other.id,
                    after: texture.id,
                });
                state.insert(TextureRef::Transient(texture.id), ResourceState::Undefined);
            }
        }
        let wanted: Vec<(TextureRef, ResourceState)> = pass
            .reads
            .iter()
            .filter(|read| !matches!(read, TextureRef::Persistent(_)))
            .map(|read| (*read, pass.kind.read_state()))
            .chain(
                pass.writes
                    .iter()
                    .map(|write| (*write, pass.kind.write_state())),
            )
            .collect();
        for (resource, to) in wanted {
            let from = state
                .get(&resource)
                .copied()
                .unwrap_or(ResourceState::Undefined);
            if from != to {
                barriers.push(Barrier::Transition { resource, from, to });
                state.insert(resource, to);
            }
        }
        pass.barriers = barriers;
    }
    let surface = state
        .get(&TextureRef::Surface)
        .copied()
        .unwrap_or(ResourceState::Present);
    match surface {
        ResourceState::Present => Vec::new(),
        from => vec![Barrier::Transition {
            resource: TextureRef::Surface,
            from,
            to: ResourceState::Present,
        }],
    }
}
