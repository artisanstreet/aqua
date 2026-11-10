use aqua::render::cull::cull;
use aqua_core::{Oklch, Rect};
use aqua_graph::{Glyph, GlyphRun, PaintCommand, Quad, Shadow, ShadowCast, TextureHandle};

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn quad(x: f32, y: f32) -> PaintCommand {
    PaintCommand::Quad(Quad {
        bounds: rect(x, y, 10.0, 10.0),
        radius: 0.0,
        border_width: 0.0,
        fill: Oklch::from_hex("#ffffff").unwrap(),
        border_color: Oklch::TRANSPARENT,
    })
}

fn glyph(x: f32) -> Glyph {
    Glyph {
        bounds: rect(x, 0.0, 8.0, 8.0),
        texels: rect(0.0, 0.0, 1.0, 1.0),
        color: Oklch::from_hex("#ffffff").unwrap(),
    }
}

const REGION: Rect = Rect {
    x: 100.0,
    y: 100.0,
    w: 50.0,
    h: 50.0,
};

#[test]
fn drops_leaves_outside_the_region_and_keeps_those_that_touch_it() {
    let mut commands = vec![quad(0.0, 0.0), quad(120.0, 120.0), quad(148.0, 100.0)];
    let culled = cull(&mut commands, REGION);
    assert_eq!(culled, 1);
    assert_eq!(commands, vec![quad(120.0, 120.0), quad(148.0, 100.0)]);
}

#[test]
fn keeps_populated_scopes_balanced_and_drops_emptied_ones() {
    let mut commands = vec![
        PaintCommand::PushClip(rect(0.0, 0.0, 20.0, 20.0)),
        quad(0.0, 0.0),
        PaintCommand::PopClip,
        PaintCommand::PushOpacity(0.5),
        quad(120.0, 120.0),
        PaintCommand::PopOpacity,
        PaintCommand::PushClip(rect(100.0, 100.0, 20.0, 20.0)),
        PaintCommand::PushRoundedClip {
            rect: rect(0.0, 0.0, 20.0, 20.0),
            radius: 4.0,
        },
        quad(0.0, 0.0),
        PaintCommand::PopClip,
        PaintCommand::PopClip,
    ];
    let culled = cull(&mut commands, REGION);
    assert_eq!(culled, 8);
    assert_eq!(
        commands,
        vec![
            PaintCommand::PushOpacity(0.5),
            quad(120.0, 120.0),
            PaintCommand::PopOpacity,
        ]
    );
}

#[test]
fn thins_glyph_runs_and_drops_empty_ones() {
    let mut commands = vec![
        PaintCommand::Glyphs(GlyphRun {
            atlas: TextureHandle(1),
            coverage: aqua_graph::Coverage::Text,
            glyphs: vec![glyph(0.0), glyph(104.0), glyph(500.0)].into(),
        }),
        PaintCommand::Glyphs(GlyphRun {
            atlas: TextureHandle(1),
            coverage: aqua_graph::Coverage::Text,
            glyphs: vec![glyph(0.0), glyph(500.0)].into(),
        }),
    ];
    cull(&mut commands, rect(100.0, 0.0, 50.0, 50.0));
    assert_eq!(commands.len(), 1);
    let PaintCommand::Glyphs(run) = &commands[0] else {
        panic!("glyph run expected")
    };
    assert_eq!(run.glyphs, vec![glyph(104.0)]);
}

#[test]
fn shadows_reach_into_the_region_through_their_blur() {
    let shadow = |x: f32| {
        PaintCommand::Shadow(Shadow {
            bounds: rect(x, 100.0, 10.0, 10.0),
            radius: 0.0,
            offset: [0.0, 0.0],
            sigma: 10.0,
            spread: 0.0,
            color: Oklch::from_hex("#000000").unwrap(),
            cast: ShadowCast::Whole,
        })
    };
    let mut commands = vec![shadow(90.0 - 20.0), shadow(0.0)];
    cull(&mut commands, REGION);
    assert_eq!(commands.len(), 1);
}

#[test]
fn never_culls_inside_an_effect_layer() {
    let mut commands = vec![
        PaintCommand::PushContentBlur {
            bounds: rect(0.0, 0.0, 400.0, 400.0),
            radius: 6.0,
        },
        quad(0.0, 0.0),
        PaintCommand::PopContentBlur,
        quad(0.0, 0.0),
    ];
    cull(&mut commands, REGION);
    assert_eq!(commands.len(), 3);
}
