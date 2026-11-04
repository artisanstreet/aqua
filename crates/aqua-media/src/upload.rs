use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_graph::{
    Extent, GlyphInstance, ImageInstance, Pass, PassKind, TextureFormat, TextureHandle, TextureRef,
};
use aqua_protocol::{TextureOp, TextureRegion, TextureUpload, UploadBatch};

use crate::atlas::{AtlasUpdates, PageFormat, PageId, Placement};

pub fn texture_handle(page: PageId) -> TextureHandle {
    let kind = match page.format {
        PageFormat::Mask8 => 0,
        PageFormat::Color8 => 1,
    };
    TextureHandle(page.index * 2 + kind)
}

pub fn texture_format(format: PageFormat) -> TextureFormat {
    match format {
        PageFormat::Mask8 => TextureFormat::R8Unorm,
        PageFormat::Color8 => TextureFormat::Rgba8Unorm,
    }
}

pub fn upload_batch(updates: AtlasUpdates) -> UploadBatch {
    let released = updates.released.into_iter().map(|page| TextureUpload {
        handle: texture_handle(page),
        op: TextureOp::Release,
    });
    let created = updates.created.into_iter().map(|create| TextureUpload {
        handle: texture_handle(create.id),
        op: TextureOp::Create {
            extent: Extent {
                width: create.width,
                height: create.height,
            },
            format: texture_format(create.id.format),
        },
    });
    let written = updates.uploads.into_iter().map(|upload| TextureUpload {
        handle: texture_handle(upload.page),
        op: TextureOp::Write {
            region: TextureRegion {
                x: upload.x,
                y: upload.y,
                extent: Extent {
                    width: upload.width,
                    height: upload.height,
                },
            },
            data: upload.data,
        },
    });
    UploadBatch {
        textures: released.chain(created).chain(written).collect(),
    }
}

fn texel_rect(placement: &Placement) -> Rect {
    Rect {
        x: placement.x as f32,
        y: placement.y as f32,
        w: placement.width as f32,
        h: placement.height as f32,
    }
}

pub fn glyph_instance(
    placement: &Placement,
    pen: [i32; 2],
    color: Oklch,
    clip: Option<Rect>,
    opacity: f32,
) -> GlyphInstance {
    GlyphInstance {
        bounds: Rect {
            x: (pen[0] + placement.bearing[0]) as f32,
            y: (pen[1] - placement.bearing[1]) as f32,
            w: placement.width as f32,
            h: placement.height as f32,
        },
        texels: texel_rect(placement),
        color,
        clip,
        opacity,
    }
}

pub fn image_instance(
    placement: &Placement,
    bounds: Rect,
    radius: f32,
    opacity: f32,
    clip: Option<Rect>,
) -> ImageInstance {
    ImageInstance {
        source: TextureRef::Persistent(texture_handle(placement.page)),
        bounds,
        texels: texel_rect(placement),
        radius,
        opacity,
        clip,
    }
}

pub fn glyph_passes(
    instances: impl IntoIterator<Item = (PageId, GlyphInstance)>,
    target: TextureRef,
) -> Vec<Pass> {
    let mut runs: Vec<(PageId, Vec<GlyphInstance>)> = Vec::new();
    for (page, instance) in instances {
        match runs.last_mut() {
            Some((last, run)) if *last == page => run.push(instance),
            _ => runs.push((page, vec![instance])),
        }
    }
    runs.into_iter()
        .map(|(page, instances)| {
            let atlas = texture_handle(page);
            Pass::new(
                PassKind::Glyphs { atlas, instances },
                vec![TextureRef::Persistent(atlas)],
                vec![target],
            )
        })
        .collect()
}

pub fn image_pass(instances: Vec<ImageInstance>, target: TextureRef) -> Pass {
    let mut reads: Vec<TextureRef> = instances.iter().map(|image| image.source).collect();
    reads.sort_by_key(|read| match read {
        TextureRef::Surface => (0u8, 0u32),
        TextureRef::Transient(id) => (1, id.0),
        TextureRef::Persistent(handle) => (2, handle.0),
    });
    reads.dedup();
    Pass::new(PassKind::Images { instances }, reads, vec![target])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageWrite {
    pub page: PageId,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct PageTable {
    edge: u32,
    known: std::collections::HashSet<PageId>,
}

impl PageTable {
    pub fn new(edge: u32) -> Self {
        Self {
            edge,
            known: std::collections::HashSet::new(),
        }
    }

    pub fn known_pages(&self) -> usize {
        self.known.len()
    }

    pub fn batch(&mut self, writes: impl IntoIterator<Item = PageWrite>) -> UploadBatch {
        let mut textures = Vec::new();
        for write in writes {
            if self.known.insert(write.page) {
                textures.push(TextureUpload {
                    handle: texture_handle(write.page),
                    op: TextureOp::Create {
                        extent: Extent {
                            width: self.edge,
                            height: self.edge,
                        },
                        format: texture_format(write.page.format),
                    },
                });
            }
            textures.push(TextureUpload {
                handle: texture_handle(write.page),
                op: TextureOp::Write {
                    region: TextureRegion {
                        x: write.x,
                        y: write.y,
                        extent: Extent {
                            width: write.width,
                            height: write.height,
                        },
                    },
                    data: write.data,
                },
            });
        }
        UploadBatch { textures }
    }

    pub fn forget_all(&mut self) {
        self.known.clear();
    }
}
