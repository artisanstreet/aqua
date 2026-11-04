use aqua_core::geometry::{CornerRadii, Rect};
use aqua_core::scene::{DisplayItem, ShadowLayer};
use aqua_graph::{FramePlan, Pass, PassKind, QuadInstance, ShadowInstance, TextureRef};
use aqua_protocol::GpuError;

fn rounded(radius: f32) -> CornerRadii {
    CornerRadii::uniform(radius)
}

fn lower_quad(instance: &QuadInstance, items: &mut Vec<DisplayItem>) -> Result<(), GpuError> {
    if instance.gradient.is_some() {
        return Err(GpuError::Unsupported("gradient fills"));
    }
    let wrapped = instance.opacity < 1.0;
    if wrapped {
        items.push(DisplayItem::PushOpacity(instance.opacity));
    }
    if let Some(clip) = instance.clip {
        items.push(DisplayItem::PushClip {
            bounds: clip,
            radius: 0.0,
        });
    }
    items.push(DisplayItem::Rect {
        bounds: instance.bounds,
        radius: instance.radius,
        fill: instance.fill,
        border_width: instance.border_width,
        border_color: instance.border_color,
        shadow: None,
    });
    if instance.clip.is_some() {
        items.push(DisplayItem::PopClip);
    }
    if wrapped {
        items.push(DisplayItem::PopOpacity);
    }
    Ok(())
}

fn lower_shadow(instance: &ShadowInstance, items: &mut Vec<DisplayItem>) {
    let wrapped = instance.opacity < 1.0;
    if wrapped {
        items.push(DisplayItem::PushOpacity(instance.opacity));
    }
    if let Some(clip) = instance.clip {
        items.push(DisplayItem::PushClip {
            bounds: clip,
            radius: 0.0,
        });
    }
    items.push(DisplayItem::Shadow {
        bounds: instance.bounds,
        corners: rounded(instance.radius),
        layer: ShadowLayer {
            dx: instance.offset[0],
            dy: instance.offset[1],
            blur: instance.sigma * 2.0,
            spread: instance.spread,
            inset: false,
            color: instance.color,
        },
    });
    if instance.clip.is_some() {
        items.push(DisplayItem::PopClip);
    }
    if wrapped {
        items.push(DisplayItem::PopOpacity);
    }
}

fn lower_target(
    passes: &[Pass],
    target: TextureRef,
    items: &mut Vec<DisplayItem>,
) -> Result<(), GpuError> {
    for pass in passes
        .iter()
        .filter(|pass| pass.writes.first() == Some(&target))
    {
        match &pass.kind {
            PassKind::Instanced { instances, .. } => {
                for instance in instances {
                    lower_quad(instance, items)?;
                }
            }
            PassKind::Shadows { instances } => {
                instances
                    .iter()
                    .for_each(|instance| lower_shadow(instance, items));
            }
            PassKind::Fullscreen { opacity, .. } => {
                if let Some(source) = pass.reads.first() {
                    items.push(DisplayItem::PushOpacity(*opacity));
                    lower_target(passes, *source, items)?;
                    items.push(DisplayItem::PopOpacity);
                }
            }
            PassKind::Glyphs { .. } => return Err(GpuError::Unsupported("glyph atlas passes")),
            PassKind::Images { .. } => return Err(GpuError::Unsupported("image passes")),
            PassKind::Filter { .. } => return Err(GpuError::Unsupported("filter passes")),
            PassKind::Compute { .. } => return Err(GpuError::Unsupported("compute passes")),
            PassKind::Copy { .. } => return Err(GpuError::Unsupported("copy passes")),
            PassKind::Custom(_) => return Err(GpuError::Unsupported("custom passes")),
        }
    }
    Ok(())
}

pub fn lower(plan: &FramePlan) -> Result<Vec<DisplayItem>, GpuError> {
    let mut items = Vec::new();
    if plan.clear.alpha > 0.0 {
        items.push(DisplayItem::Rect {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: plan.extent.width as f32,
                h: plan.extent.height as f32,
            },
            radius: 0.0,
            fill: plan.clear,
            border_width: 0.0,
            border_color: plan.clear,
            shadow: None,
        });
    }
    lower_target(&plan.passes, TextureRef::Surface, &mut items)?;
    Ok(items)
}
