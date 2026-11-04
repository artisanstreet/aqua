use std::ops::Range;

use aqua_graph::{ImageInstance, TextureRef};
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

pub fn glyph_layout() -> [D3D12_INPUT_ELEMENT_DESC; 4] {
    [
        element(s!("BOUNDS"), 0),
        element(s!("TEXELS"), 1),
        element(s!("CLIP"), 2),
        element(s!("COLOR"), 3),
    ]
}

pub fn image_layout() -> [D3D12_INPUT_ELEMENT_DESC; 4] {
    [
        element(s!("BOUNDS"), 0),
        element(s!("TEXELS"), 1),
        element(s!("CLIP"), 2),
        element(s!("PARAMS"), 3),
    ]
}

pub fn image_runs(instances: &[ImageInstance]) -> Vec<(TextureRef, Range<u32>)> {
    let mut runs: Vec<(TextureRef, Range<u32>)> = Vec::new();
    for (index, image) in instances.iter().enumerate() {
        match runs.last_mut() {
            Some((last, range)) if *last == image.source && range.end == index as u32 => {
                range.end += 1
            }
            _ => runs.push((image.source, index as u32..index as u32 + 1)),
        }
    }
    runs
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
