use std::collections::HashMap;
use std::sync::Arc;

use aqua_graph::{FramePlan, TransientId};
use objc2_metal::MTLTextureUsage;

use super::formats::metal;
use super::Error;
use crate::hal::{Device, Texture as GpuTexture, TextureDesc};
use crate::ledger::Kind;
use crate::lifetimes::{assign_with, shapes, Precision, Shape};

const IDLE_FRAMES: u32 = 60;

pub struct Texture {
    pub texture: GpuTexture,
    shape: Shape,
    claimed: bool,
    idle: u32,
}

pub struct Transients {
    ceiling: Option<u64>,
    precision: Precision,
    pool: Vec<Texture>,
    keys: Vec<(TransientId, Shape)>,
    index: HashMap<TransientId, usize>,
}

fn create(device: &Arc<Device>, shape: Shape) -> Result<Texture, Error> {
    let usage = MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead;
    let texture = GpuTexture::new(
        device,
        TextureDesc {
            extent: shape.extent,
            format: metal(shape.format),
            usage: match shape.unordered {
                true => usage | MTLTextureUsage::ShaderWrite,
                false => usage,
            },
            label: "transient_texture",
            kind: Kind::TransientTexture,
            bytes: shape.bytes(),
        },
    )?;
    Ok(Texture {
        texture,
        shape,
        claimed: false,
        idle: 0,
    })
}

impl Transients {
    pub fn new(ceiling: Option<u64>) -> Self {
        Self {
            ceiling,
            precision: Precision::Planned,
            pool: Vec::new(),
            keys: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn bytes(&self) -> u64 {
        self.pool.iter().map(|texture| texture.shape.bytes()).sum()
    }

    pub fn textures(&self) -> usize {
        self.pool.len()
    }

    pub fn get(&self, id: TransientId) -> Option<&Texture> {
        self.index.get(&id).map(|position| &self.pool[*position])
    }

    pub fn trim(&mut self) -> u64 {
        let freed = self.bytes();
        self.pool.clear();
        self.keys.clear();
        self.index.clear();
        freed
    }

    fn matches(&self, plan: &FramePlan) -> bool {
        self.keys.len() == plan.textures.len()
            && self.keys.iter().copied().eq(shapes(plan, self.precision))
    }

    pub fn ensure(
        &mut self,
        device: &Arc<Device>,
        plan: &FramePlan,
        precision: Precision,
    ) -> Result<bool, Error> {
        self.precision = precision;
        let created = match self.matches(plan) {
            true => false,
            false => self.reassign(device, plan)?,
        };
        self.age();
        Ok(created)
    }

    fn reassign(&mut self, device: &Arc<Device>, plan: &FramePlan) -> Result<bool, Error> {
        let assignment = assign_with(plan, self.precision);
        if let Some(ceiling) = self.ceiling.filter(|ceiling| assignment.bytes() > *ceiling) {
            return Err(Error::Budget {
                needed: assignment.bytes(),
                ceiling,
            });
        }
        self.pool
            .iter_mut()
            .for_each(|texture| texture.claimed = false);
        let mut created = false;
        let mut slots = Vec::with_capacity(assignment.slots.len());
        for shape in &assignment.slots {
            let found = self
                .pool
                .iter()
                .position(|texture| !texture.claimed && texture.shape == *shape);
            let position = match found {
                Some(position) => position,
                None => {
                    self.pool.push(create(device, *shape)?);
                    created = true;
                    self.pool.len() - 1
                }
            };
            self.pool[position].claimed = true;
            self.pool[position].idle = 0;
            slots.push(position);
        }
        self.keys = shapes(plan, self.precision).collect();
        self.index = self
            .keys
            .iter()
            .zip(&assignment.slot_of)
            .map(|((id, _), slot)| (*id, slots[*slot]))
            .collect();
        Ok(created)
    }

    fn age(&mut self) {
        let mut idle_bytes = 0u64;
        let mut live_bytes = 0u64;
        let mut expired = false;
        for texture in &mut self.pool {
            match texture.claimed {
                true => live_bytes += texture.shape.bytes(),
                false => {
                    texture.idle += 1;
                    idle_bytes += texture.shape.bytes();
                    expired |= texture.idle >= IDLE_FRAMES;
                }
            }
        }
        if expired || idle_bytes > live_bytes {
            self.release_idle(live_bytes);
        }
    }

    fn release_idle(&mut self, live_bytes: u64) {
        let mut idle: Vec<(u32, usize)> = self
            .pool
            .iter()
            .enumerate()
            .filter(|(_, texture)| !texture.claimed)
            .map(|(position, texture)| (texture.idle, position))
            .collect();
        idle.sort_by(|a, b| b.cmp(a));
        let mut kept_bytes: u64 = idle
            .iter()
            .map(|(_, position)| self.pool[*position].shape.bytes())
            .sum();
        let mut dropped = vec![false; self.pool.len()];
        for (frames, position) in idle {
            if frames >= IDLE_FRAMES || kept_bytes > live_bytes {
                kept_bytes -= self.pool[position].shape.bytes();
                dropped[position] = true;
            }
        }
        let mut remap = vec![usize::MAX; self.pool.len()];
        let mut next = 0;
        for (position, gone) in dropped.iter().enumerate() {
            if !gone {
                remap[position] = next;
                next += 1;
            }
        }
        let mut position = 0;
        self.pool.retain(|_| {
            let keep = !dropped[position];
            position += 1;
            keep
        });
        self.index
            .values_mut()
            .for_each(|position| *position = remap[*position]);
    }
}
