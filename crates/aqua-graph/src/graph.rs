use std::collections::{HashMap, HashSet};

use aqua_core::{Oklch, geometry::Rect};

use crate::analysis::{
    Lifetime, align_up, first_writers, independent, intersect, lifetimes, union,
};
use crate::plan::{
    Barrier, Extent, FramePlan, GlyphInstance, LoadOp, Pass, PassKind, QuadInstance, ResourceState,
    ShaderId, ShadowInstance, TextureFormat, TextureHandle, TextureRef, TransientId,
    TransientTexture,
};
use crate::slug::SlugInstance;
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
    extents: Vec<Option<Rect>>,
    merged: usize,
    glyph_scratch: Vec<GlyphInstance>,
    slug_scratch: Vec<SlugInstance>,
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
            extents: Vec::new(),
            merged: 0,
            glyph_scratch: Vec::new(),
            slug_scratch: Vec::new(),
        }
    }

    pub fn create(&mut self, extent: Extent, format: TextureFormat) -> TextureRef {
        let id = TransientId(self.resources.len() as u32);
        self.resources.push(ResourceDesc { extent, format });
        TextureRef::Transient(id)
    }

    pub fn push(&mut self, pass: Pass) {
        match batchable(&pass) {
            true => self.merged += usize::from(place(&mut self.passes, &mut self.extents, pass)),
            false => {
                self.extents.push(drawn_extent(&pass));
                self.passes.push(pass);
            }
        }
    }

    pub fn draw_quad(&mut self, target: TextureRef, quad: QuadInstance) {
        let extent = clip_to(quad.covered(), quad.clip);
        let tag = Tag::Quad;
        let found = self.find_batch(target, tag, extent);
        match (found, &mut self.passes) {
            (Some(index), passes) => {
                if let PassKind::Instanced { instances, .. } = &mut passes[index].kind {
                    instances.push(quad);
                }
                self.merged += 1;
                self.extents[index] = grown(self.extents[index], extent);
            }
            _ => self.start_batch(
                PassKind::Instanced {
                    shader: ShaderId::Quad,
                    instances: vec![quad],
                },
                Vec::new(),
                target,
                extent,
            ),
        }
    }

    pub fn draw_shadow(&mut self, target: TextureRef, shadow: ShadowInstance) {
        let extent = clip_to(shadow.covered(), shadow.clip);
        let found = self.find_batch(target, Tag::Shadow, extent);
        match found {
            Some(index) => {
                if let PassKind::Shadows { instances } = &mut self.passes[index].kind {
                    instances.push(shadow);
                }
                self.merged += 1;
                self.extents[index] = grown(self.extents[index], extent);
            }
            None => self.start_batch(
                PassKind::Shadows {
                    instances: vec![shadow],
                },
                Vec::new(),
                target,
                extent,
            ),
        }
    }

    pub fn draw_glyphs(
        &mut self,
        target: TextureRef,
        atlas: TextureHandle,
        glyphs: impl Iterator<Item = GlyphInstance>,
    ) {
        let mut scratch = std::mem::take(&mut self.glyph_scratch);
        scratch.clear();
        scratch.extend(glyphs);
        let extent = fold_extent(scratch.iter().map(|g| clip_to(g.bounds, g.clip))).flatten();
        let found = self.find_batch(target, Tag::Glyphs(atlas), extent);
        match found {
            Some(index) => {
                if let PassKind::Glyphs { instances, .. } = &mut self.passes[index].kind {
                    instances.extend_from_slice(&scratch);
                }
                self.merged += 1;
                self.extents[index] = grown(self.extents[index], extent);
            }
            None => self.start_batch(
                PassKind::Glyphs {
                    atlas,
                    instances: scratch.clone(),
                },
                vec![TextureRef::Persistent(atlas)],
                target,
                extent,
            ),
        }
        self.glyph_scratch = scratch;
    }

    pub fn draw_slug(
        &mut self,
        target: TextureRef,
        store: TextureHandle,
        glyphs: impl Iterator<Item = SlugInstance>,
    ) {
        let mut scratch = std::mem::take(&mut self.slug_scratch);
        scratch.clear();
        scratch.extend(glyphs);
        let extent = fold_extent(scratch.iter().map(|g| clip_to(g.covered(), g.clip))).flatten();
        let found = self.find_batch(target, Tag::Slug(store), extent);
        match found {
            Some(index) => {
                if let PassKind::Slug { instances, .. } = &mut self.passes[index].kind {
                    instances.extend_from_slice(&scratch);
                }
                self.merged += 1;
                self.extents[index] = grown(self.extents[index], extent);
            }
            None => self.start_batch(
                PassKind::Slug {
                    store,
                    instances: scratch.clone(),
                },
                vec![TextureRef::Persistent(store)],
                target,
                extent,
            ),
        }
        self.slug_scratch = scratch;
    }

    fn start_batch(
        &mut self,
        kind: PassKind,
        reads: Vec<TextureRef>,
        target: TextureRef,
        extent: Option<Rect>,
    ) {
        self.passes.push(Pass::new(kind, reads, vec![target]));
        self.extents.push(extent);
    }

    fn find_batch(&self, target: TextureRef, tag: Tag, extent: Option<Rect>) -> Option<usize> {
        for (index, earlier) in self.passes.iter().enumerate().rev() {
            let same_target = earlier.writes.as_slice() == [target];
            if same_target
                && earlier.load == LoadOp::Load
                && earlier.constants.is_empty()
                && tag.matches(&earlier.kind)
            {
                return Some(index);
            }
            let touches = earlier.writes.contains(&target) || earlier.reads.contains(&target);
            let disjoint = same_target
                && !earlier.reads.contains(&target)
                && matches!((self.extents[index], extent), (Some(a), Some(b)) if intersect(a, b).is_none());
            if touches && !disjoint {
                return None;
            }
        }
        None
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

    pub fn plan_with_stats(
        mut self,
        limits: PlanLimits,
    ) -> Result<(FramePlan, PlanStats), PlanError> {
        let mut stats = PlanStats::default();
        self.check_resources(limits)?;
        let mut passes = std::mem::take(&mut self.passes);
        stats.culled_instances = cull_instances(&self, &mut passes);
        passes = drop_unfed(passes);
        let before = passes.len();
        passes = cull_passes(passes);
        stats.culled_passes = before - passes.len();
        let before = passes.len();
        passes = merge_passes(passes);
        stats.merged_passes = before - passes.len() + self.merged;
        ensure_surface_clear(&mut passes, self.clear);
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
                paints && visible(quad.covered(), quad.clip, target)
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
            PassKind::Images { instances } => instances.retain(|image| {
                image.opacity > 0.0 && visible(image.covered(), image.clip, target)
            }),
            PassKind::Slug { instances, .. } => instances.retain(|glyph| {
                glyph.opacity > 0.0
                    && glyph.color.alpha > 0.0
                    && visible(glyph.covered(), glyph.clip, target)
            }),
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
                | PassKind::Slug { .. }
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
            (PassKind::Slug { store: x, .. }, PassKind::Slug { store: y, .. }) => x == y,
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
        (
            PassKind::Slug { instances, .. },
            PassKind::Slug {
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

#[derive(Clone, Copy)]
enum Tag {
    Quad,
    Shadow,
    Glyphs(TextureHandle),
    Slug(TextureHandle),
}

impl Tag {
    fn matches(self, kind: &PassKind) -> bool {
        match (self, kind) {
            (
                Tag::Quad,
                PassKind::Instanced {
                    shader: ShaderId::Quad,
                    ..
                },
            ) => true,
            (Tag::Shadow, PassKind::Shadows { .. }) => true,
            (Tag::Glyphs(a), PassKind::Glyphs { atlas, .. }) => a == *atlas,
            (Tag::Slug(a), PassKind::Slug { store, .. }) => a == *store,
            _ => false,
        }
    }
}

fn clip_to(covered: Rect, clip: Option<Rect>) -> Option<Rect> {
    match clip {
        Some(clip) => intersect(covered, clip),
        None => Some(covered),
    }
}

fn fold_extent(rects: impl Iterator<Item = Option<Rect>>) -> Option<Option<Rect>> {
    rects.into_iter().try_fold(None::<Rect>, |acc, rect| {
        rect.map(|rect| Some(acc.map_or(rect, |acc| union(acc, rect))))
    })
}

fn drawn_extent(pass: &Pass) -> Option<Rect> {
    let clipped = |covered: Rect, clip: Option<Rect>| match clip {
        Some(clip) => intersect(covered, clip),
        None => Some(covered),
    };
    let extent = match &pass.kind {
        PassKind::Instanced { instances, .. } => {
            fold_extent(instances.iter().map(|q| clipped(q.covered(), q.clip)))
        }
        PassKind::Shadows { instances } => {
            fold_extent(instances.iter().map(|s| clipped(s.covered(), s.clip)))
        }
        PassKind::Glyphs { instances, .. } => {
            fold_extent(instances.iter().map(|g| clipped(g.bounds, g.clip)))
        }
        PassKind::Images { instances } => {
            fold_extent(instances.iter().map(|i| clipped(i.covered(), i.clip)))
        }
        PassKind::Slug { instances, .. } => {
            fold_extent(instances.iter().map(|g| clipped(g.covered(), g.clip)))
        }
        _ => None,
    };
    extent.flatten()
}

fn spatially_disjoint(
    earlier: &Pass,
    earlier_extent: Option<Rect>,
    pass: &Pass,
    extent: Option<Rect>,
) -> bool {
    let (Some(a), Some(b)) = (earlier_extent, extent) else {
        return false;
    };
    earlier.writes == pass.writes
        && earlier.writes.len() == 1
        && !earlier.reads.contains(&earlier.writes[0])
        && !pass.reads.contains(&pass.writes[0])
        && intersect(a, b).is_none()
}

fn batchable(pass: &Pass) -> bool {
    matches!(
        pass.kind,
        PassKind::Instanced { .. }
            | PassKind::Shadows { .. }
            | PassKind::Glyphs { .. }
            | PassKind::Slug { .. }
    ) && pass
        .reads
        .iter()
        .all(|read| !matches!(read, TextureRef::Transient(_)))
}

fn grown(existing: Option<Rect>, added: Option<Rect>) -> Option<Rect> {
    match (existing, added) {
        (Some(a), Some(b)) => Some(union(a, b)),
        _ => None,
    }
}

fn merge_target(
    result: &[Pass],
    extents: &[Option<Rect>],
    pass: &Pass,
    extent: Option<Rect>,
) -> Option<usize> {
    for (index, earlier) in result.iter().enumerate().rev() {
        if same_batch(earlier, pass) {
            return Some(index);
        }
        if !independent(earlier, pass) && !spatially_disjoint(earlier, extents[index], pass, extent)
        {
            return None;
        }
    }
    None
}

fn place(passes: &mut Vec<Pass>, extents: &mut Vec<Option<Rect>>, pass: Pass) -> bool {
    let extent = drawn_extent(&pass);
    match merge_target(passes, extents, &pass, extent) {
        Some(index) => {
            extents[index] = grown(extents[index], extent);
            absorb(&mut passes[index], pass);
            true
        }
        None => {
            passes.push(pass);
            extents.push(extent);
            false
        }
    }
}

fn merge_passes(passes: Vec<Pass>) -> Vec<Pass> {
    let mut result = Vec::with_capacity(passes.len());
    let mut extents = Vec::with_capacity(passes.len());
    passes.into_iter().for_each(|pass| {
        place(&mut result, &mut extents, pass);
    });
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

fn ensure_surface_clear(passes: &mut Vec<Pass>, clear: Oklch) {
    let first_write = passes
        .iter()
        .position(|pass| pass.writes.contains(&TextureRef::Surface));
    let first_read = passes
        .iter()
        .position(|pass| pass.reads.contains(&TextureRef::Surface));
    let uncleared = match (first_write, first_read) {
        (None, _) => true,
        (Some(write), Some(read)) => read < write,
        (Some(_), None) => false,
    };
    if uncleared {
        passes.insert(0, Pass::clear(TextureRef::Surface, clear));
    }
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
