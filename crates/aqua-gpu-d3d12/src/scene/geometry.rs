use std::sync::Arc;

use aqua_core::Rect;
use aqua_graph::{FramePlan, PassKind};
use windows::Win32::Graphics::Direct3D12::*;

use super::util::{create_buffer, map_whole};
use super::Error;
use crate::ledger::{record, Kind, Token};

pub(super) struct Geometry {
    pub bounds: Arc<[Rect]>,
    pub buffer: ID3D12Resource,
    _token: Token,
}

pub(super) fn retain(
    device: &ID3D12Device,
    geometries: &mut Vec<Geometry>,
    plan: &FramePlan,
) -> Result<u64, Error> {
    geometries.retain(|geometry| plan.passes.iter().any(|pass| {
        matches!(&pass.kind, PassKind::SrgbaSolids { bounds, .. } if Arc::ptr_eq(bounds, &geometry.bounds))
    }));
    let mut uploaded = 0;
    for pass in &plan.passes {
        let PassKind::SrgbaSolids { bounds, .. } = &pass.kind else {
            continue;
        };
        if bounds.is_empty()
            || geometries
                .iter()
                .any(|geometry| Arc::ptr_eq(bounds, &geometry.bounds))
        {
            continue;
        }
        let size = bounds.len() as u64 * 16;
        let buffer = create_buffer(
            device,
            D3D12_HEAP_TYPE_UPLOAD,
            size,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        )?;
        let mapped = map_whole(&buffer)?.cast::<[f32; 4]>();
        for (index, rect) in bounds.iter().enumerate() {
            unsafe {
                mapped.add(index).write([rect.x, rect.y, rect.w, rect.h]);
            }
        }
        unsafe {
            buffer.Unmap(0, None);
        }
        geometries.push(Geometry {
            bounds: Arc::clone(bounds),
            buffer,
            _token: record("solid_bounds", Kind::UploadBuffer, size),
        });
        uploaded += size;
    }
    Ok(uploaded)
}
