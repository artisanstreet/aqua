use std::collections::HashMap;

use aqua_graph::{Extent, FramePlan, LoadOp, Pass, PassKind, ShaderId, TextureRef, TransientId};
use aqua_protocol::GpuError;

use crate::filter::{blur_down, blur_up};
use crate::raster::{Layer, composite_layer, draw_quad, premultiplied};
use crate::sample::draw_image;
use crate::shadow::draw_shadow;
use crate::sprites::{Textures, draw_glyphs, draw_images};

pub struct Frame<'a> {
    plan: &'a FramePlan,
    persistent: &'a Textures,
    surface: Option<Layer>,
    transients: HashMap<TransientId, Layer>,
}

fn failure(detail: impl Into<String>) -> GpuError {
    GpuError::DeviceFailure(detail.into())
}

fn layer_of(extent: Extent) -> Layer {
    Layer::transparent(extent.width as usize, extent.height as usize)
}

impl<'a> Frame<'a> {
    pub fn new(plan: &'a FramePlan, persistent: &'a Textures) -> Self {
        let clear = premultiplied(plan.clear);
        Self {
            plan,
            persistent,
            surface: Some(Layer::filled(
                plan.extent.width as usize,
                plan.extent.height as usize,
                clear,
            )),
            transients: HashMap::new(),
        }
    }

    fn extent_of(&self, reference: TextureRef) -> Result<Extent, GpuError> {
        match reference {
            TextureRef::Surface => Ok(self.plan.extent),
            TextureRef::Transient(id) => self
                .plan
                .texture(id)
                .map(|texture| texture.extent)
                .ok_or_else(|| failure(format!("unknown transient {}", id.0))),
            TextureRef::Persistent(handle) => self
                .persistent
                .get(handle)
                .map(|texture| texture.extent)
                .ok_or_else(|| failure(format!("unknown persistent texture {}", handle.0))),
        }
    }

    fn read(&self, reference: TextureRef) -> Result<&Layer, GpuError> {
        match reference {
            TextureRef::Surface => self
                .surface
                .as_ref()
                .ok_or_else(|| failure("surface is being written")),
            TextureRef::Transient(id) => self
                .transients
                .get(&id)
                .ok_or_else(|| failure(format!("transient {} was never written", id.0))),
            TextureRef::Persistent(handle) => Err(failure(format!(
                "persistent texture {} is not a render layer",
                handle.0
            ))),
        }
    }

    fn take(&mut self, reference: TextureRef) -> Result<Layer, GpuError> {
        let extent = self.extent_of(reference)?;
        match reference {
            TextureRef::Surface => self
                .surface
                .take()
                .ok_or_else(|| failure("surface is being written")),
            TextureRef::Transient(id) => Ok(self
                .transients
                .remove(&id)
                .unwrap_or_else(|| layer_of(extent))),
            TextureRef::Persistent(_) => Err(failure("persistent textures are not render targets")),
        }
    }

    fn restore(&mut self, reference: TextureRef, layer: Layer) {
        match reference {
            TextureRef::Surface => self.surface = Some(layer),
            TextureRef::Transient(id) => {
                self.transients.insert(id, layer);
            }
            TextureRef::Persistent(_) => {}
        }
    }

    pub fn run(mut self) -> Result<Layer, GpuError> {
        for pass in &self.plan.passes {
            let [target_ref] = pass.writes.as_slice() else {
                return Err(GpuError::Unsupported(
                    "passes with several or no render targets",
                ));
            };
            let mut target = self.take(*target_ref)?;
            match pass.load {
                LoadOp::Clear(color) => {
                    let fill = premultiplied(color);
                    target.pixels.iter_mut().for_each(|pixel| *pixel = fill);
                }
                LoadOp::Load | LoadOp::DontCare => {}
            }
            let outcome = self.execute(pass, &mut target);
            self.restore(*target_ref, target);
            outcome?;
        }
        self.surface.ok_or_else(|| failure("surface was lost"))
    }

    fn execute(&self, pass: &Pass, target: &mut Layer) -> Result<(), GpuError> {
        match &pass.kind {
            PassKind::Instanced {
                shader: ShaderId::Quad,
                instances,
            } => {
                instances
                    .iter()
                    .for_each(|instance| draw_quad(target, instance));
                Ok(())
            }
            PassKind::Instanced { .. } => Err(GpuError::Unsupported(
                "instanced shaders other than the quad",
            )),
            PassKind::Shadows { instances } => {
                instances
                    .iter()
                    .for_each(|instance| draw_shadow(target, instance));
                Ok(())
            }
            PassKind::Glyphs { atlas, instances } => {
                let atlas = self
                    .persistent
                    .get(*atlas)
                    .ok_or_else(|| failure(format!("unknown atlas {}", atlas.0)))?;
                draw_glyphs(target, atlas, instances);
                Ok(())
            }
            PassKind::Images { instances } => {
                for image in instances {
                    match image.source {
                        TextureRef::Persistent(handle) => {
                            self.persistent.get(handle).ok_or_else(|| {
                                failure(format!("unknown image texture {}", handle.0))
                            })?;
                            draw_images(target, self.persistent, std::slice::from_ref(image));
                        }
                        source => draw_image(target, self.read(source)?, image),
                    }
                }
                Ok(())
            }
            PassKind::Fullscreen {
                shader: ShaderId::LayerOver,
                opacity,
            } => {
                let source = self.read(
                    *pass
                        .reads
                        .first()
                        .ok_or_else(|| failure("layer pass without a source"))?,
                )?;
                match (source.width == target.width, source.height == target.height) {
                    (true, true) => {
                        composite_layer(target, source, *opacity);
                        Ok(())
                    }
                    _ => Err(failure("layer and target extents differ")),
                }
            }
            PassKind::Fullscreen { .. } => Err(GpuError::Unsupported(
                "fullscreen shaders other than layer-over",
            )),
            PassKind::Filter { shader, params } => {
                let source = self.read(
                    *pass
                        .reads
                        .first()
                        .ok_or_else(|| failure("filter pass without a source"))?,
                )?;
                match shader {
                    ShaderId::BlurDown => {
                        blur_down(source, target, params);
                        Ok(())
                    }
                    ShaderId::BlurUp => {
                        blur_up(source, target, params);
                        Ok(())
                    }
                    _ => Err(GpuError::Unsupported(
                        "filter shaders other than the blur pyramid",
                    )),
                }
            }
            PassKind::Copy {
                source_origin,
                dest_origin,
                size,
            } => {
                let source = self.read(
                    *pass
                        .reads
                        .first()
                        .ok_or_else(|| failure("copy without a source"))?,
                )?;
                let fits = |origin: [u32; 2], width: usize, height: usize| {
                    (origin[0] + size.width) as usize <= width
                        && (origin[1] + size.height) as usize <= height
                };
                match fits(*source_origin, source.width, source.height)
                    && fits(*dest_origin, target.width, target.height)
                {
                    true => {
                        for row in 0..size.height as usize {
                            for column in 0..size.width as usize {
                                let pixel = source.at(
                                    source_origin[0] as usize + column,
                                    source_origin[1] as usize + row,
                                );
                                target.put(
                                    dest_origin[0] as usize + column,
                                    dest_origin[1] as usize + row,
                                    pixel,
                                );
                            }
                        }
                        Ok(())
                    }
                    false => Err(failure("copy region exceeds a texture")),
                }
            }
            PassKind::Compute { .. } => Err(GpuError::Unsupported("compute passes")),
            PassKind::Custom(_) => Err(GpuError::Unsupported("custom passes")),
        }
    }
}
