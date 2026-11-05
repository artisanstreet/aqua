use aqua_core::Oklch;
use aqua_gpu_d3d11::lifetimes::{Shape, assign};
use aqua_graph::{
    CustomShaderId, Extent, FramePlan, Pass, PassKind, ShaderId, TextureFormat, TextureRef,
    ToneCurve, TransientId, TransientTexture,
};

const SMALL: Extent = Extent {
    width: 64,
    height: 32,
};
const LARGE: Extent = Extent {
    width: 128,
    height: 64,
};

fn texture(id: u32, extent: Extent, format: TextureFormat) -> TransientTexture {
    TransientTexture {
        id: TransientId(id),
        extent,
        format,
        heap_offset: 0,
    }
}

fn draw(reads: &[u32], writes: &[TextureRef]) -> Pass {
    Pass::new(
        PassKind::Instanced {
            shader: ShaderId::Quad,
            instances: Vec::new(),
        },
        reads
            .iter()
            .map(|id| TextureRef::Transient(TransientId(*id)))
            .collect(),
        writes.to_vec(),
    )
}

fn layer(id: u32) -> TextureRef {
    TextureRef::Transient(TransientId(id))
}

fn plan(textures: Vec<TransientTexture>, passes: Vec<Pass>) -> FramePlan {
    FramePlan {
        extent: LARGE,
        clear: Oklch::TRANSPARENT,
        textures,
        passes,
        heap_bytes: 0,
        final_barriers: Vec::new(),
        text_tone: ToneCurve::native(),
    }
}

fn shape(extent: Extent, format: TextureFormat, unordered: bool) -> Shape {
    Shape {
        extent,
        format,
        unordered,
    }
}

#[test]
fn transients_with_disjoint_lifetimes_and_equal_shapes_share_one_texture() {
    let plan = plan(
        vec![
            texture(1, SMALL, TextureFormat::Rgba16Float),
            texture(2, SMALL, TextureFormat::Rgba16Float),
        ],
        vec![
            draw(&[], &[layer(1)]),
            draw(&[1], &[TextureRef::Surface]),
            draw(&[], &[layer(2)]),
            draw(&[2], &[TextureRef::Surface]),
        ],
    );
    let assignment = assign(&plan);
    assert_eq!(
        assignment.slots,
        vec![shape(SMALL, TextureFormat::Rgba16Float, false)]
    );
    assert_eq!(assignment.slot_of, vec![0, 0]);
    assert_eq!(assignment.bytes(), SMALL.texels() * 8);
}

#[test]
fn overlapping_lifetimes_never_share_even_with_equal_shapes() {
    let plan = plan(
        vec![
            texture(1, SMALL, TextureFormat::Rgba16Float),
            texture(2, SMALL, TextureFormat::Rgba16Float),
            texture(3, SMALL, TextureFormat::Rgba16Float),
        ],
        vec![
            draw(&[], &[layer(1)]),
            draw(&[1], &[layer(2)]),
            draw(&[2], &[layer(3)]),
            draw(&[3], &[TextureRef::Surface]),
        ],
    );
    let assignment = assign(&plan);
    assert_eq!(assignment.slots.len(), 2);
    assert_eq!(assignment.slot_of, vec![0, 1, 0]);
}

#[test]
fn a_pass_that_reads_one_transient_and_writes_another_keeps_them_apart() {
    let plan = plan(
        vec![
            texture(1, SMALL, TextureFormat::Rgba16Float),
            texture(2, SMALL, TextureFormat::Rgba16Float),
        ],
        vec![
            draw(&[], &[layer(1)]),
            draw(&[1], &[layer(2)]),
            draw(&[2], &[TextureRef::Surface]),
        ],
    );
    let assignment = assign(&plan);
    assert_ne!(assignment.slot_of[0], assignment.slot_of[1]);
}

#[test]
fn shapes_differ_by_extent_format_and_compute_writes() {
    let compute = Pass::new(
        PassKind::Compute {
            shader: ShaderId::Custom(CustomShaderId(1)),
            groups: [1, 1, 1],
            params: [0.0; 4],
        },
        vec![layer(1)],
        vec![layer(4)],
    );
    let plan = plan(
        vec![
            texture(1, SMALL, TextureFormat::Rgba16Float),
            texture(2, LARGE, TextureFormat::Rgba16Float),
            texture(3, SMALL, TextureFormat::Rgba8Unorm),
            texture(4, SMALL, TextureFormat::Rgba16Float),
        ],
        vec![
            draw(&[], &[layer(1)]),
            draw(&[1], &[TextureRef::Surface]),
            draw(&[], &[layer(2)]),
            draw(&[2], &[TextureRef::Surface]),
            draw(&[], &[layer(3)]),
            draw(&[3], &[TextureRef::Surface]),
            draw(&[], &[layer(1)]),
            compute,
            draw(&[4], &[TextureRef::Surface]),
        ],
    );
    let assignment = assign(&plan);
    assert_eq!(
        assignment.slots,
        vec![
            shape(SMALL, TextureFormat::Rgba16Float, false),
            shape(LARGE, TextureFormat::Rgba16Float, false),
            shape(SMALL, TextureFormat::Rgba8Unorm, false),
            shape(SMALL, TextureFormat::Rgba16Float, true),
        ]
    );
    assert_eq!(assignment.slot_of, vec![0, 1, 2, 3]);
}

#[test]
fn reuse_picks_the_first_free_slot_in_order_of_first_use() {
    let plan = plan(
        vec![
            texture(5, SMALL, TextureFormat::R8Unorm),
            texture(6, SMALL, TextureFormat::R8Unorm),
            texture(7, SMALL, TextureFormat::R8Unorm),
        ],
        vec![
            draw(&[], &[layer(6)]),
            draw(&[], &[layer(5)]),
            draw(&[6], &[TextureRef::Surface]),
            draw(&[5], &[TextureRef::Surface]),
            draw(&[], &[layer(7)]),
            draw(&[7], &[TextureRef::Surface]),
        ],
    );
    let assignment = assign(&plan);
    assert_eq!(assignment.slots.len(), 2);
    assert_eq!(assignment.slot_of, vec![1, 0, 0]);
}
