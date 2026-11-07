use aqua::render::namespace::{
    namespace_plan, namespace_uploads, Namespaces, NAMESPACE_LIMIT, NAMESPACE_SHIFT,
};
use aqua_core::{Oklch, Rect};
use aqua_graph::{
    plan_paint, Extent, Glyph, GlyphRun, Image, PaintCommand, PassKind, TextureHandle, TextureRef,
};
use aqua_protocol::{TextureOp, TextureUpload, UploadBatch};

fn rect() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        w: 8.0,
        h: 8.0,
    }
}

fn persistent_handles(plan: &aqua_graph::FramePlan) -> Vec<u32> {
    let mut found = Vec::new();
    for pass in &plan.passes {
        for texture in pass.reads.iter().chain(pass.writes.iter()) {
            if let TextureRef::Persistent(handle) = texture {
                found.push(handle.0);
            }
        }
        match &pass.kind {
            PassKind::Glyphs { atlas, .. } => found.push(atlas.0),
            PassKind::Images { instances } => {
                for image in instances {
                    if let TextureRef::Persistent(handle) = image.source {
                        found.push(handle.0);
                    }
                }
            }
            _ => {}
        }
    }
    found
}

fn plan() -> aqua_graph::FramePlan {
    let commands = vec![
        PaintCommand::Glyphs(GlyphRun {
            atlas: TextureHandle(2),
            coverage: aqua_graph::Coverage::Text,
            glyphs: vec![Glyph {
                bounds: rect(),
                texels: rect(),
                color: Oklch::BLACK,
            }]
            .into(),
        }),
        PaintCommand::Image(Image {
            source: TextureHandle(5),
            bounds: rect(),
            texels: rect(),
            radius: 0.0,
        }),
    ];
    plan_paint(
        Extent {
            width: 64,
            height: 64,
        },
        Oklch::BLACK,
        &commands,
    )
    .unwrap()
}

#[test]
fn every_persistent_reference_moves_into_the_namespace() {
    let mut shifted = plan();
    let original = persistent_handles(&shifted);
    assert!(original.contains(&2) && original.contains(&5));
    namespace_plan(3, &mut shifted);
    let moved = persistent_handles(&shifted);
    assert_eq!(moved.len(), original.len());
    assert!(moved.iter().all(|handle| handle >> NAMESPACE_SHIFT == 3));
    let stripped: Vec<u32> = moved
        .iter()
        .map(|handle| handle & ((1 << NAMESPACE_SHIFT) - 1))
        .collect();
    assert_eq!(stripped, original);
}

#[test]
fn namespace_zero_leaves_plans_and_uploads_untouched() {
    let mut untouched = plan();
    namespace_plan(0, &mut untouched);
    assert_eq!(untouched, plan());
    let mut batch = UploadBatch {
        textures: vec![TextureUpload {
            handle: TextureHandle(7),
            op: TextureOp::Release,
        }],
    };
    namespace_uploads(0, &mut batch);
    assert_eq!(batch.textures[0].handle, TextureHandle(7));
    namespace_uploads(2, &mut batch);
    assert_eq!(
        batch.textures[0].handle,
        TextureHandle(7 | (2 << NAMESPACE_SHIFT))
    );
}

#[test]
fn released_namespaces_are_reused_lowest_first() {
    let mut namespaces = Namespaces::default();
    let first = namespaces.acquire().unwrap();
    let second = namespaces.acquire().unwrap();
    let third = namespaces.acquire().unwrap();
    assert_eq!((first, second, third), (0, 1, 2));
    namespaces.release(1);
    assert_eq!(namespaces.acquire(), Some(1));
    assert_eq!(namespaces.acquire(), Some(3));
}

#[test]
fn namespaces_run_out_at_the_limit() {
    let mut namespaces = Namespaces::default();
    let all: Vec<_> = (0..NAMESPACE_LIMIT).map(|_| namespaces.acquire()).collect();
    assert!(all.iter().all(Option::is_some));
    assert_eq!(namespaces.acquire(), None);
}
