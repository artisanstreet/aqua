use aqua_graph::analysis::lifetime_of;
use aqua_graph::{Extent, FramePlan, PassKind, TextureFormat, TextureRef, TransientId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    pub extent: Extent,
    pub format: TextureFormat,
    pub unordered: bool,
}

impl Shape {
    pub fn bytes(&self) -> u64 {
        self.extent.texels() * self.format.bytes_per_texel()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Assignment {
    pub slots: Vec<Shape>,
    pub slot_of: Vec<usize>,
}

impl Assignment {
    pub fn bytes(&self) -> u64 {
        self.slots.iter().map(Shape::bytes).sum()
    }
}

fn unordered_write(plan: &FramePlan, id: TransientId) -> bool {
    plan.passes.iter().any(|pass| {
        matches!(pass.kind, PassKind::Compute { .. })
            && pass.writes.contains(&TextureRef::Transient(id))
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Precision {
    #[default]
    Planned,
    EightBit,
}

fn stored_format(format: TextureFormat, unordered: bool, precision: Precision) -> TextureFormat {
    match (precision, format, unordered) {
        (Precision::EightBit, TextureFormat::Rgba16Float, false) => TextureFormat::Bgra8Unorm,
        _ => format,
    }
}

pub fn shapes(
    plan: &FramePlan,
    precision: Precision,
) -> impl Iterator<Item = (TransientId, Shape)> + '_ {
    plan.textures.iter().map(move |texture| {
        let unordered = unordered_write(plan, texture.id);
        (
            texture.id,
            Shape {
                extent: texture.extent,
                format: stored_format(texture.format, unordered, precision),
                unordered,
            },
        )
    })
}

pub fn assign(plan: &FramePlan) -> Assignment {
    assign_with(plan, Precision::Planned)
}

pub fn assign_with(plan: &FramePlan, precision: Precision) -> Assignment {
    let unused = plan.passes.len();
    let mut order: Vec<(usize, usize, usize, Shape)> = shapes(plan, precision)
        .enumerate()
        .map(|(index, (id, shape))| {
            let (first, last) = lifetime_of(&plan.passes, id)
                .map_or((unused, unused), |lifetime| (lifetime.first, lifetime.last));
            (first, last, index, shape)
        })
        .collect();
    order.sort_by_key(|(first, _, index, _)| (*first, *index));
    let mut busy_until: Vec<usize> = Vec::with_capacity(order.len());
    let mut assignment = Assignment {
        slots: Vec::with_capacity(order.len()),
        slot_of: vec![0; order.len()],
    };
    for (first, last, index, shape) in order {
        let free = (0..assignment.slots.len())
            .find(|slot| assignment.slots[*slot] == shape && busy_until[*slot] < first);
        let slot = match free {
            Some(slot) => {
                busy_until[slot] = last;
                slot
            }
            None => {
                assignment.slots.push(shape);
                busy_until.push(last);
                assignment.slots.len() - 1
            }
        };
        assignment.slot_of[index] = slot;
    }
    assignment
}
