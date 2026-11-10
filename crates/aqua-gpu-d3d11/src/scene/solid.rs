use std::sync::Arc;

use aqua_core::Rect;
use aqua_graph::{FramePlan, PassKind};
use windows::Win32::Graphics::Direct3D11::*;

use super::Error;
use crate::ledger::{record, Kind, Token};

pub(super) struct Geometry {
    pub bounds: Arc<[Rect]>,
    pub buffer: ID3D11Buffer,
    _token: Token,
}

pub(super) fn retain(
    device: &ID3D11Device,
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
        let packed = bounds
            .iter()
            .map(|rect| [rect.x, rect.y, rect.w, rect.h])
            .collect::<Vec<_>>();
        let size = u32::try_from(packed.len() * 16)
            .map_err(|_| Error::Unsupported("solid geometry capacity"))?;
        let description = D3D11_BUFFER_DESC {
            ByteWidth: size,
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
            ..Default::default()
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: packed.as_ptr().cast(),
            ..Default::default()
        };
        let mut buffer = None;
        unsafe {
            device.CreateBuffer(&description, Some(&data), Some(&mut buffer))?;
        }
        geometries.push(Geometry {
            bounds: Arc::clone(bounds),
            buffer: buffer.ok_or(Error::NoAdapter)?,
            _token: record("solid_bounds", Kind::VertexBuffer, u64::from(size)),
        });
        uploaded += u64::from(size);
    }
    Ok(uploaded)
}

