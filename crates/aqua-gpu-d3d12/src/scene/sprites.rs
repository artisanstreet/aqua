use std::ops::Range;

use aqua_graph::{Blend, ImageInstance, TextureRef};
use windows::core::{s, PCSTR};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::sprite::SPRITE_STRIDE;

fn element(name: PCSTR, index: u32) -> D3D12_INPUT_ELEMENT_DESC {
    D3D12_INPUT_ELEMENT_DESC {
        SemanticName: name,
        SemanticIndex: 0,
        Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
        InputSlot: 0,
        AlignedByteOffset: index * 16,
        InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
        InstanceDataStepRate: 1,
    }
}

pub fn glyph_layout() -> [D3D12_INPUT_ELEMENT_DESC; 5] {
    [
        element(s!("BOUNDS"), 0),
        element(s!("TEXELS"), 1),
        element(s!("CLIP"), 2),
        element(s!("COLOR"), 3),
        element(s!("EXTRA"), 4),
    ]
}

pub fn image_layout() -> [D3D12_INPUT_ELEMENT_DESC; 5] {
    [
        element(s!("BOUNDS"), 0),
        element(s!("TEXELS"), 1),
        element(s!("CLIP"), 2),
        element(s!("PARAMS"), 3),
        element(s!("XFORM"), 4),
    ]
}

pub fn slug_layout() -> [D3D12_INPUT_ELEMENT_DESC; 5] {
    [
        element(s!("PLACEMENT"), 0),
        element(s!("GLYPH"), 1),
        element(s!("EMBOUNDS"), 2),
        element(s!("CLIP"), 3),
        element(s!("COLOR"), 4),
    ]
}

pub fn image_runs(
    instances: &[ImageInstance],
) -> impl Iterator<Item = (TextureRef, Blend, Range<u32>)> + '_ {
    let mut index = 0usize;
    std::iter::from_fn(move || {
        let first = instances.get(index)?;
        let start = index;
        while instances
            .get(index)
            .is_some_and(|next| next.source == first.source && next.blend == first.blend)
        {
            index += 1;
        }
        Some((first.source, first.blend, start as u32..index as u32))
    })
}

pub fn draw(list: &ID3D12GraphicsCommandList, buffer: u64, range: Range<u32>) {
    unsafe {
        list.IASetVertexBuffers(
            0,
            Some(&[D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: buffer + range.start as u64 * SPRITE_STRIDE as u64,
                SizeInBytes: (range.end - range.start) * SPRITE_STRIDE as u32,
                StrideInBytes: SPRITE_STRIDE as u32,
            }]),
        );
        list.DrawInstanced(4, range.end - range.start, 0, 0);
    }
}
