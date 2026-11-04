use std::collections::HashMap;

use aqua_graph::{CustomPassSpec, CustomShaderId};

use crate::raster::{Layer, Pixel};

pub const INVERT: &str = "aqua.invert";
pub const ROW_BOX_BLUR: CustomShaderId = CustomShaderId(1);
pub const ROW_BOX_BLUR_TILE: u32 = 64;

pub struct ComputeCall {
    pub groups: [u32; 3],
    pub params: [f32; 4],
}

pub type CustomKernel = Box<dyn Fn(&CustomPassSpec, &[&Layer], &mut Layer) + Send + Sync>;
pub type ComputeKernel = Box<dyn Fn(&ComputeCall, &[&Layer], &mut Layer) + Send + Sync>;

#[derive(Default)]
pub struct Kernels {
    custom: HashMap<String, CustomKernel>,
    compute: HashMap<CustomShaderId, ComputeKernel>,
}

impl Kernels {
    pub fn standard() -> Self {
        let mut kernels = Self::default();
        kernels.register_custom(INVERT, Box::new(invert));
        kernels.register_compute(ROW_BOX_BLUR, Box::new(row_box_blur));
        kernels
    }

    pub fn register_custom(&mut self, name: impl Into<String>, kernel: CustomKernel) {
        self.custom.insert(name.into(), kernel);
    }

    pub fn register_compute(&mut self, id: CustomShaderId, kernel: ComputeKernel) {
        self.compute.insert(id, kernel);
    }

    pub fn custom(&self, name: &str) -> Option<&CustomKernel> {
        self.custom.get(name)
    }

    pub fn compute(&self, id: CustomShaderId) -> Option<&ComputeKernel> {
        self.compute.get(&id)
    }
}

fn invert(spec: &CustomPassSpec, inputs: &[&Layer], target: &mut Layer) {
    let amount = spec.params.first().copied().unwrap_or(1.0).clamp(0.0, 1.0);
    let Some(source) = inputs.first() else {
        return;
    };
    if source.width != target.width || source.height != target.height {
        return;
    }
    for (pixel, out) in source.pixels.iter().zip(target.pixels.iter_mut()) {
        let alpha = pixel[3];
        let mapped: Pixel = match alpha > 0.0 {
            true => {
                let straight = [pixel[0] / alpha, pixel[1] / alpha, pixel[2] / alpha];
                let flipped = straight.map(|c| c + (1.0 - 2.0 * c) * amount);
                [
                    flipped[0] * alpha,
                    flipped[1] * alpha,
                    flipped[2] * alpha,
                    alpha,
                ]
            }
            false => [0.0; 4],
        };
        *out = mapped;
    }
}

fn row_box_blur(call: &ComputeCall, inputs: &[&Layer], target: &mut Layer) {
    let Some(source) = inputs.first() else {
        return;
    };
    if source.width != target.width || source.height != target.height || source.width == 0 {
        return;
    }
    let reach = call.params[0].max(0.0).round() as i64;
    let columns = ((call.groups[0] * ROW_BOX_BLUR_TILE) as usize).min(target.width);
    let rows = (call.groups[1] as usize).min(target.height);
    for y in 0..rows {
        for x in 0..columns {
            let mut sum = [0.0f32; 4];
            for offset in -reach..=reach {
                let tap = (x as i64 + offset).clamp(0, source.width as i64 - 1) as usize;
                let pixel = source.at(tap, y);
                (0..4).for_each(|channel| sum[channel] += pixel[channel]);
            }
            let count = (2 * reach + 1) as f32;
            target.put(x, y, sum.map(|channel| channel / count));
        }
    }
}
