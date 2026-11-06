#![cfg(windows)]

use aqua_gpu_d3d11::scene::{read_texture, PersistentTextures};
use aqua_graph::{Extent, TextureFormat, TextureHandle};
use aqua_protocol::{TextureOp, TextureRegion, TextureUpload, UploadBatch};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
use windows::Win32::Graphics::Direct3D11::*;

const EXTENT: Extent = Extent {
    width: 16,
    height: 8,
};

fn device() -> (ID3D11Device, ID3D11DeviceContext1) {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_WARP,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .unwrap()
    };
    (device.unwrap(), context.unwrap().cast().unwrap())
}

fn batch(ops: Vec<(u32, TextureOp)>) -> Vec<UploadBatch> {
    vec![UploadBatch {
        textures: ops
            .into_iter()
            .map(|(handle, op)| TextureUpload {
                handle: TextureHandle(handle),
                op,
            })
            .collect(),
    }]
}

fn create(extent: Extent) -> TextureOp {
    TextureOp::Create {
        extent,
        format: TextureFormat::R8Unorm,
    }
}

fn fill(value: u8) -> TextureOp {
    TextureOp::Write {
        region: TextureRegion {
            x: 0,
            y: 0,
            extent: EXTENT,
        },
        data: vec![value; (EXTENT.width * EXTENT.height) as usize],
    }
}

fn contents(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext1,
    textures: &PersistentTextures,
    handle: u32,
) -> Vec<u8> {
    let entry = textures.get(TextureHandle(handle)).unwrap();
    read_texture(device, context, &entry.texture, |bytes, pitch| {
        (0..EXTENT.height as usize)
            .flat_map(|row| bytes[row * pitch..row * pitch + EXTENT.width as usize].to_vec())
            .collect()
    })
    .unwrap()
}

#[test]
fn a_released_texture_is_reused_for_the_same_shape_and_comes_back_zeroed() {
    let (device, context) = device();
    let mut textures = PersistentTextures::new(&device);
    textures
        .apply(
            &context,
            &batch(vec![
                (1, create(EXTENT)),
                (1, fill(0xAB)),
                (1, TextureOp::Release),
            ]),
        )
        .unwrap();
    assert_eq!(textures.stats().pooled_textures, 1);
    textures
        .apply(&context, &batch(vec![(2, create(EXTENT))]))
        .unwrap();
    let stats = textures.stats();
    assert_eq!((stats.reused, stats.pooled_textures), (1, 0));
    assert!(contents(&device, &context, &textures, 2)
        .iter()
        .all(|byte| *byte == 0));
}

#[test]
fn a_different_shape_creates_a_fresh_texture_and_draining_empties_the_pool() {
    let (device, context) = device();
    let mut textures = PersistentTextures::new(&device);
    let other = Extent {
        width: 32,
        height: 8,
    };
    textures
        .apply(
            &context,
            &batch(vec![
                (1, create(EXTENT)),
                (1, TextureOp::Release),
                (2, create(other)),
            ]),
        )
        .unwrap();
    let stats = textures.stats();
    assert_eq!((stats.reused, stats.pooled_textures), (0, 1));
    assert_eq!(stats.pooled_bytes, 16 * 8);
    assert_eq!(textures.drain_pool(), 1);
    assert_eq!(textures.stats().pooled_bytes, 0);
}
