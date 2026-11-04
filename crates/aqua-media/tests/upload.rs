use aqua_core::Oklch;
use aqua_graph::{PassKind, TextureRef};
use aqua_media::atlas::{AtlasUpdates, PageCreate, PageFormat, PageId, Placement, TexelUpload};
use aqua_media::upload::{glyph_instance, glyph_passes, image_pass, texture_handle, upload_batch};
use aqua_protocol::TextureOp;

fn page(format: PageFormat, index: u32) -> PageId {
    PageId { format, index }
}

fn placement(page: PageId) -> Placement {
    Placement {
        page,
        x: 4,
        y: 8,
        width: 6,
        height: 9,
        page_width: 64,
        page_height: 64,
        bearing: [1, 8],
    }
}

#[test]
fn mask_and_colour_pages_with_the_same_index_get_distinct_handles() {
    let handles: Vec<_> = (0..4)
        .flat_map(|index| {
            [
                texture_handle(page(PageFormat::Mask8, index)),
                texture_handle(page(PageFormat::Color8, index)),
            ]
        })
        .collect();
    let mut unique = handles.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), handles.len());
}

#[test]
fn batches_release_before_create_before_write_so_reused_page_indices_stay_valid() {
    let id = page(PageFormat::Mask8, 0);
    let updates = AtlasUpdates {
        released: vec![id],
        created: vec![PageCreate {
            id,
            width: 64,
            height: 64,
        }],
        uploads: vec![TexelUpload {
            page: id,
            x: 1,
            y: 2,
            width: 2,
            height: 1,
            data: vec![9, 9],
        }],
    };
    let batch = upload_batch(updates);
    let ops: Vec<_> = batch
        .textures
        .iter()
        .map(|upload| std::mem::discriminant(&upload.op))
        .collect();
    assert_eq!(ops[0], std::mem::discriminant(&TextureOp::Release));
    assert!(matches!(batch.textures[1].op, TextureOp::Create { .. }));
    assert!(matches!(batch.textures[2].op, TextureOp::Write { .. }));
    assert_eq!(batch.upload_bytes(), 2);
}

#[test]
fn glyph_bounds_hang_from_the_pen_by_the_bearing() {
    let instance = glyph_instance(
        &placement(page(PageFormat::Mask8, 0)),
        [100, 50],
        Oklch::WHITE,
        None,
        1.0,
    );
    assert_eq!((instance.bounds.x, instance.bounds.y), (101.0, 42.0));
    assert_eq!(
        (
            instance.texels.x,
            instance.texels.y,
            instance.texels.w,
            instance.texels.h
        ),
        (4.0, 8.0, 6.0, 9.0)
    );
}

#[test]
fn glyph_runs_split_only_where_the_page_changes() {
    let first = page(PageFormat::Mask8, 0);
    let second = page(PageFormat::Mask8, 1);
    let instance = |page| {
        (
            page,
            glyph_instance(&placement(page), [0, 0], Oklch::WHITE, None, 1.0),
        )
    };
    let passes = glyph_passes(
        [
            instance(first),
            instance(first),
            instance(second),
            instance(first),
        ],
        TextureRef::Surface,
    );
    let sizes: Vec<usize> = passes
        .iter()
        .map(|pass| match &pass.kind {
            PassKind::Glyphs { instances, .. } => instances.len(),
            _ => 0,
        })
        .collect();
    assert_eq!(sizes, vec![2, 1, 1]);
    assert_eq!(
        passes[0].reads,
        vec![TextureRef::Persistent(texture_handle(first))]
    );
}

#[test]
fn image_passes_declare_each_source_once_in_sorted_order() {
    let low = placement(page(PageFormat::Color8, 0));
    let high = placement(page(PageFormat::Color8, 3));
    let rect = aqua_core::geometry::Rect {
        x: 0.0,
        y: 0.0,
        w: 4.0,
        h: 4.0,
    };
    let image =
        |placement: &Placement| aqua_media::upload::image_instance(placement, rect, 0.0, 1.0, None);
    let pass = image_pass(
        vec![image(&high), image(&low), image(&high)],
        TextureRef::Surface,
    );
    assert_eq!(
        pass.reads,
        vec![
            TextureRef::Persistent(texture_handle(low.page)),
            TextureRef::Persistent(texture_handle(high.page)),
        ]
    );
}

#[test]
fn foreign_atlas_writes_create_each_page_exactly_once_before_its_first_write() {
    use aqua_media::upload::{PageTable, PageWrite};
    let first = page(PageFormat::Mask8, 0);
    let second = page(PageFormat::Color8, 0);
    let write = |page: PageId, x: u32| PageWrite {
        page,
        x,
        y: 0,
        width: 2,
        height: 1,
        data: vec![1; 2 * page.format.bytes_per_texel() as usize],
    };
    let mut table = PageTable::new(512);
    let batch = table.batch([write(first, 0), write(first, 4), write(second, 0)]);
    let kinds: Vec<&str> = batch
        .textures
        .iter()
        .map(|upload| match upload.op {
            TextureOp::Create { .. } => "create",
            TextureOp::Write { .. } => "write",
            TextureOp::Release => "release",
        })
        .collect();
    assert_eq!(kinds, vec!["create", "write", "write", "create", "write"]);
    assert_eq!(table.batch([write(first, 8)]).textures.len(), 1);
    table.forget_all();
    assert_eq!(table.batch([write(first, 8)]).textures.len(), 2);
    assert_eq!(table.known_pages(), 1);
}
