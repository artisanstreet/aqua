use aqua_core::geometry::Rect;
use aqua_graph::PaintCommand;

const EDGE_PAD: f32 = 2.0;
const SHADOW_SIGMAS: f32 = 3.0;

fn grown(rect: Rect, by: f32) -> Rect {
    Rect {
        x: rect.x - by,
        y: rect.y - by,
        w: rect.w + 2.0 * by,
        h: rect.h + 2.0 * by,
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

fn shadow_reach(shadow: &aqua_graph::Shadow) -> Rect {
    let spread = shadow.spread.max(0.0);
    let reach = shadow.sigma.abs() * SHADOW_SIGMAS + spread;
    let moved = Rect {
        x: shadow.bounds.x + shadow.offset[0],
        y: shadow.bounds.y + shadow.offset[1],
        ..shadow.bounds
    };
    grown(moved, reach)
}

enum Verdict {
    Keep,
    Drop,
    Thin,
}

fn judge(command: &PaintCommand, region: Rect) -> Verdict {
    let hit = |rect: Rect| match overlaps(grown(rect, EDGE_PAD), region) {
        true => Verdict::Keep,
        false => Verdict::Drop,
    };
    match command {
        PaintCommand::Quad(quad) => hit(quad.bounds),
        PaintCommand::GradientQuad(item) => hit(item.quad.bounds),
        PaintCommand::Shadow(shadow) => hit(shadow_reach(shadow)),
        PaintCommand::Image(image) => hit(image.bounds),
        PaintCommand::BackdropBlur(blur) => hit(blur.bounds),
        PaintCommand::Glyphs(_) => Verdict::Thin,
        _ => Verdict::Keep,
    }
}

fn opens_layer(command: &PaintCommand) -> bool {
    matches!(
        command,
        PaintCommand::PushContentBlur { .. }
            | PaintCommand::PushColorMatrix { .. }
            | PaintCommand::PushCustomEffect { .. }
    )
}

fn closes_layer(command: &PaintCommand) -> bool {
    matches!(
        command,
        PaintCommand::PopContentBlur | PaintCommand::PopColorMatrix | PaintCommand::PopCustomEffect
    )
}

fn closes_empty(open: &PaintCommand, close: &PaintCommand) -> bool {
    matches!(
        (open, close),
        (
            PaintCommand::PushClip(_) | PaintCommand::PushRoundedClip { .. },
            PaintCommand::PopClip
        ) | (PaintCommand::PushOpacity(_), PaintCommand::PopOpacity)
    )
}

fn collapse_empty_scopes(commands: &mut Vec<PaintCommand>) {
    let mut kept = 0usize;
    for index in 0..commands.len() {
        let empties = kept > 0 && closes_empty(&commands[kept - 1], &commands[index]);
        match empties {
            true => kept -= 1,
            false => {
                commands.swap(kept, index);
                kept += 1;
            }
        }
    }
    commands.truncate(kept);
}

pub fn cull(commands: &mut Vec<PaintCommand>, region: Rect) -> u32 {
    let before = commands.len();
    let mut layers = 0usize;
    commands.retain_mut(|command| {
        match (opens_layer(command), closes_layer(command)) {
            (true, _) => layers += 1,
            (_, true) => layers = layers.saturating_sub(1),
            _ => {}
        }
        match (layers > 0, judge(command, region)) {
            (true, _) | (_, Verdict::Keep) => true,
            (_, Verdict::Drop) => false,
            (_, Verdict::Thin) => match command {
                PaintCommand::Glyphs(run) => {
                    run.glyphs
                        .retain(|glyph| overlaps(grown(glyph.bounds, EDGE_PAD), region));
                    !run.glyphs.is_empty()
                }
                _ => true,
            },
        }
    });
    collapse_empty_scopes(commands);
    (before - commands.len()) as u32
}
