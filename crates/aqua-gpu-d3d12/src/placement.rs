#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Planned {
    pub offset: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub offsets: Vec<u64>,
    pub total: u64,
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

pub fn place(planned: &[Planned], real_bytes: &[u64], alignment: u64) -> Placement {
    let mut order: Vec<usize> = (0..planned.len()).collect();
    order.sort_by_key(|&index| (planned[index].offset, index));
    let mut offsets = vec![0u64; planned.len()];
    let mut placed: Vec<usize> = Vec::with_capacity(planned.len());
    for index in order {
        let floor = placed
            .iter()
            .filter(|&&other| planned[other].offset + planned[other].bytes <= planned[index].offset)
            .map(|&other| offsets[other] + real_bytes[other])
            .max()
            .unwrap_or(0);
        offsets[index] = align_up(floor, alignment);
        placed.push(index);
    }
    let total = (0..planned.len())
        .map(|index| offsets[index] + real_bytes[index])
        .max()
        .map(|end| align_up(end, alignment))
        .unwrap_or(0);
    Placement { offsets, total }
}
