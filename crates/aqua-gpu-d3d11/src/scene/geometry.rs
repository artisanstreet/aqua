use aqua_graph::{
    FramePlan, GlyphInstance, ImageInstance, PassKind, QuadInstance, ShadowInstance, SlugInstance,
};

const BYTE_LIMIT: usize = 4 << 20;

enum Geometry {
    Empty,
    Quads(Vec<QuadInstance>),
    Shadows(Vec<ShadowInstance>),
    Glyphs(Vec<GlyphInstance>),
    Images(Vec<ImageInstance>),
    Curves(Vec<SlugInstance>),
}

impl Geometry {
    fn matches(&self, kind: &PassKind) -> bool {
        match (self, kind) {
            (Self::Quads(before), PassKind::Instanced { instances, .. }) => before == instances,
            (Self::Shadows(before), PassKind::Shadows { instances }) => before == instances,
            (Self::Glyphs(before), PassKind::Glyphs { instances, .. }) => before == instances,
            (Self::Images(before), PassKind::Images { instances }) => before == instances,
            (Self::Curves(before), PassKind::Slug { instances, .. }) => before == instances,
            (
                Self::Empty,
                PassKind::Fullscreen { .. }
                | PassKind::Filter { .. }
                | PassKind::Compute { .. }
                | PassKind::Copy { .. }
                | PassKind::Custom(_),
            ) => true,
            _ => false,
        }
    }

    fn snapshot(kind: &PassKind) -> Self {
        match kind {
            PassKind::Instanced { instances, .. } => Self::Quads(instances.clone()),
            PassKind::Shadows { instances } => Self::Shadows(instances.clone()),
            PassKind::Glyphs { instances, .. } => Self::Glyphs(instances.clone()),
            PassKind::Images { instances } => Self::Images(instances.clone()),
            PassKind::Slug { instances, .. } => Self::Curves(instances.clone()),
            _ => Self::Empty,
        }
    }
}

fn bytes(kind: &PassKind) -> usize {
    match kind {
        PassKind::Instanced { instances, .. } => instances.len() * size_of::<QuadInstance>(),
        PassKind::Shadows { instances } => instances.len() * size_of::<ShadowInstance>(),
        PassKind::Glyphs { instances, .. } => instances.len() * size_of::<GlyphInstance>(),
        PassKind::Images { instances } => instances.len() * size_of::<ImageInstance>(),
        PassKind::Slug { instances, .. } => instances.len() * size_of::<SlugInstance>(),
        _ => 0,
    }
}

#[derive(Default)]
pub(crate) struct GeometryCache {
    passes: Vec<Geometry>,
    valid: bool,
}

impl GeometryCache {
    pub fn prepare(&mut self, plan: &FramePlan) -> bool {
        if self.valid
            && self.passes.len() == plan.passes.len()
            && self
                .passes
                .iter()
                .zip(&plan.passes)
                .all(|(before, after)| before.matches(&after.kind))
        {
            return true;
        }
        let retained = plan
            .passes
            .iter()
            .map(|pass| bytes(&pass.kind))
            .sum::<usize>()
            .saturating_add(plan.passes.len() * size_of::<Geometry>());
        self.passes.clear();
        self.valid = retained <= BYTE_LIMIT;
        if self.valid {
            self.passes.extend(
                plan.passes
                    .iter()
                    .map(|pass| Geometry::snapshot(&pass.kind)),
            );
        } else {
            self.passes.shrink_to_fit();
        }
        false
    }
}
