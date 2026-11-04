use aqua_gpu_d3d12::window::mirror::TextureMirror;
use aqua_graph::{Extent, TextureFormat, TextureHandle};
use aqua_protocol::{TextureOp, TextureRegion, TextureUpload, UploadBatch};

fn batch(uploads: Vec<(u32, TextureOp)>) -> UploadBatch {
    UploadBatch {
        textures: uploads
            .into_iter()
            .map(|(handle, op)| TextureUpload {
                handle: TextureHandle(handle),
                op,
            })
            .collect(),
    }
}

fn create(width: u32, height: u32) -> TextureOp {
    TextureOp::Create {
        extent: Extent { width, height },
        format: TextureFormat::R8Unorm,
    }
}

fn write(x: u32, y: u32, width: u32, height: u32, value: u8) -> TextureOp {
    TextureOp::Write {
        region: TextureRegion {
            x,
            y,
            extent: Extent { width, height },
        },
        data: vec![value; (width * height) as usize],
    }
}

fn replayed_bytes(mirror: &TextureMirror, handle: u32) -> Vec<u8> {
    mirror
        .replay()
        .textures
        .into_iter()
        .filter(|upload| upload.handle == TextureHandle(handle))
        .find_map(|upload| match upload.op {
            TextureOp::Write { data, .. } => Some(data),
            _ => None,
        })
        .expect("texture replayed")
}

#[test]
fn partial_writes_land_at_their_offsets_in_the_replayed_image() {
    let mut mirror = TextureMirror::default();
    mirror.apply(&batch(vec![
        (1, create(4, 3)),
        (1, write(1, 1, 2, 2, 9)),
        (1, write(0, 0, 1, 1, 7)),
    ]));
    assert_eq!(
        replayed_bytes(&mirror, 1),
        vec![7, 0, 0, 0, 0, 9, 9, 0, 0, 9, 9, 0]
    );
}

#[test]
fn replay_creates_before_it_writes_and_is_sorted_by_handle() {
    let mut mirror = TextureMirror::default();
    mirror.apply(&batch(vec![
        (5, create(2, 2)),
        (2, create(2, 2)),
        (5, write(0, 0, 2, 2, 3)),
    ]));
    let ops: Vec<(u32, bool)> = mirror
        .replay()
        .textures
        .iter()
        .map(|upload| {
            (
                upload.handle.0,
                matches!(upload.op, TextureOp::Create { .. }),
            )
        })
        .collect();
    assert_eq!(ops, vec![(2, true), (2, false), (5, true), (5, false)]);
}

#[test]
fn released_textures_are_not_replayed_and_recreation_starts_blank() {
    let mut mirror = TextureMirror::default();
    mirror.apply(&batch(vec![(1, create(2, 2)), (1, write(0, 0, 2, 2, 5))]));
    mirror.apply(&batch(vec![(1, TextureOp::Release)]));
    assert_eq!(mirror.textures(), 0);
    mirror.apply(&batch(vec![(1, create(2, 2))]));
    assert_eq!(replayed_bytes(&mirror, 1), vec![0; 4]);
}

#[test]
fn out_of_bounds_and_mis_sized_writes_are_ignored() {
    let mut mirror = TextureMirror::default();
    mirror.apply(&batch(vec![
        (1, create(2, 2)),
        (1, write(1, 1, 2, 2, 8)),
        (
            1,
            TextureOp::Write {
                region: TextureRegion {
                    x: 0,
                    y: 0,
                    extent: Extent {
                        width: 2,
                        height: 2,
                    },
                },
                data: vec![1; 3],
            },
        ),
        (9, write(0, 0, 1, 1, 1)),
    ]));
    assert_eq!(replayed_bytes(&mirror, 1), vec![0; 4]);
    assert_eq!(mirror.textures(), 1);
    assert_eq!(mirror.resident_bytes(), 4);
}
