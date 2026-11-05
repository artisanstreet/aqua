use std::sync::OnceLock;

use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_graph::slug::{PathCommand, STORE_WIDTH, SlugGlyph, pack_glyph};
use aqua_graph::{
    Extent, FramePlan, Glyph, GlyphRun, Linear2, PaintCommand, SlugInstance, SlugRun,
    TextureFormat, TextureHandle, ToneCurve,
};
use aqua_protocol::{TextureOp, TextureRegion, TextureUpload, UploadBatch};

use super::assets::{ATLAS, standard_uploads};
use super::cases::{Case, STANDARD, build, hex, quad, rect};

pub const SLUG_STORE: TextureHandle = TextureHandle(3);

const STORE_ROWS: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlugAsset {
    pub index: u32,
    pub bounds: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct SlugAssets {
    pub ring: SlugAsset,
    pub wedge: SlugAsset,
    pub blob: SlugAsset,
    pub uploads: UploadBatch,
}

fn ring_path() -> Vec<PathCommand> {
    let circle = |radius: f32, forward: bool| {
        let k = radius * (std::f32::consts::SQRT_2 - 1.0);
        let h = radius * std::f32::consts::FRAC_1_SQRT_2;
        let c = 0.5;
        let at = |x: f32, y: f32| [c + x, c + y];
        let mut commands = vec![
            PathCommand::Move(at(radius, 0.0)),
            PathCommand::Quad(at(radius, k), at(h, h)),
            PathCommand::Quad(at(k, radius), at(0.0, radius)),
            PathCommand::Quad(at(-k, radius), at(-h, h)),
            PathCommand::Quad(at(-radius, k), at(-radius, 0.0)),
            PathCommand::Quad(at(-radius, -k), at(-h, -h)),
            PathCommand::Quad(at(-k, -radius), at(0.0, -radius)),
            PathCommand::Quad(at(k, -radius), at(h, -h)),
            PathCommand::Quad(at(radius, -k), at(radius, 0.0)),
            PathCommand::Close,
        ];
        if !forward {
            commands = vec![
                PathCommand::Move(at(radius, 0.0)),
                PathCommand::Quad(at(radius, -k), at(h, -h)),
                PathCommand::Quad(at(k, -radius), at(0.0, -radius)),
                PathCommand::Quad(at(-k, -radius), at(-h, -h)),
                PathCommand::Quad(at(-radius, -k), at(-radius, 0.0)),
                PathCommand::Quad(at(-radius, k), at(-h, h)),
                PathCommand::Quad(at(-k, radius), at(0.0, radius)),
                PathCommand::Quad(at(k, radius), at(h, h)),
                PathCommand::Quad(at(radius, k), at(radius, 0.0)),
                PathCommand::Close,
            ];
        }
        commands
    };
    let mut path = circle(0.5, true);
    path.extend(circle(0.22, false));
    path
}

fn wedge_path() -> Vec<PathCommand> {
    vec![
        PathCommand::Move([0.0, 0.0]),
        PathCommand::Line([0.35, 0.0]),
        PathCommand::Line([0.5, 0.45]),
        PathCommand::Line([0.65, 0.0]),
        PathCommand::Line([1.0, 0.0]),
        PathCommand::Line([0.62, 1.0]),
        PathCommand::Line([0.38, 1.0]),
        PathCommand::Close,
    ]
}

fn blob_path() -> Vec<PathCommand> {
    vec![
        PathCommand::Move([0.1, 0.0]),
        PathCommand::Quad([0.5, -0.2], [0.9, 0.0]),
        PathCommand::Quad([1.1, 0.4], [0.8, 0.8]),
        PathCommand::Cubic([0.6, 1.0], [0.3, 1.0], [0.1, 0.7]),
        PathCommand::Line([0.0, 0.3]),
        PathCommand::Close,
        PathCommand::Move([0.3, 0.3]),
        PathCommand::Line([0.3, 0.5]),
        PathCommand::Line([0.6, 0.5]),
        PathCommand::Line([0.6, 0.3]),
        PathCommand::Close,
    ]
}

fn build_assets() -> SlugAssets {
    let mut texels: Vec<[f32; 4]> = Vec::new();
    let mut add = |path: Vec<PathCommand>| {
        let glyph = SlugGlyph::from_path(&path).expect("conformance outline");
        let index = texels.len() as u32;
        texels.extend(pack_glyph(&glyph, index));
        SlugAsset {
            index,
            bounds: glyph.bounds,
        }
    };
    let ring = add(ring_path());
    let wedge = add(wedge_path());
    let blob = add(blob_path());
    let mut data = Vec::with_capacity(STORE_ROWS as usize * STORE_WIDTH as usize * 16);
    for texel in &texels {
        texel
            .iter()
            .for_each(|value| data.extend_from_slice(&value.to_le_bytes()));
    }
    data.resize(STORE_ROWS as usize * STORE_WIDTH as usize * 16, 0);
    let extent = Extent::new(STORE_WIDTH, STORE_ROWS);
    let mut uploads = standard_uploads();
    uploads.textures.push(TextureUpload {
        handle: SLUG_STORE,
        op: TextureOp::Create {
            extent,
            format: TextureFormat::Rgba32Float,
        },
    });
    uploads.textures.push(TextureUpload {
        handle: SLUG_STORE,
        op: TextureOp::Write {
            region: TextureRegion { x: 0, y: 0, extent },
            data,
        },
    });
    SlugAssets {
        ring,
        wedge,
        blob,
        uploads,
    }
}

pub fn slug_assets() -> &'static SlugAssets {
    static ASSETS: OnceLock<SlugAssets> = OnceLock::new();
    ASSETS.get_or_init(build_assets)
}

pub fn slug_glyph(
    asset: SlugAsset,
    origin: [f32; 2],
    pixels_per_em: f32,
    placement: Linear2,
    color: Oklch,
) -> SlugInstance {
    SlugInstance {
        origin,
        linear: Linear2::scale(pixels_per_em, -pixels_per_em).then(placement),
        glyph: asset.index,
        em_bounds: asset.bounds,
        color,
        clip: None,
        opacity: 1.0,
    }
}

fn slug_run(glyphs: Vec<SlugInstance>) -> PaintCommand {
    PaintCommand::Slug(SlugRun {
        store: SLUG_STORE,
        glyphs,
    })
}

fn atlas_run(glyphs: &[(Rect, Rect, &str, f32)]) -> PaintCommand {
    PaintCommand::Glyphs(GlyphRun {
        atlas: ATLAS,
        glyphs: glyphs
            .iter()
            .map(|(bounds, texels, color, alpha)| Glyph {
                bounds: *bounds,
                texels: *texels,
                color: hex(color, *alpha),
            })
            .collect(),
    })
}

fn slug_case(name: &str, clear: Oklch, commands: Vec<PaintCommand>) -> Case {
    Case {
        uploads: slug_assets().uploads.clone(),
        ..build(name, STANDARD, clear, &commands)
    }
}

fn dark() -> PaintCommand {
    quad(0.0, 0.0, 96.0, 72.0, "#253040", 1.0, 0.0, 0.0)
}

fn mid() -> PaintCommand {
    quad(0.0, 0.0, 96.0, 72.0, "#7f7f7f", 1.0, 0.0, 0.0)
}

fn slug_midtone(name: &str) -> Case {
    let assets = slug_assets();
    slug_case(
        name,
        Oklch::TRANSPARENT,
        vec![
            mid(),
            slug_run(vec![
                slug_glyph(
                    assets.ring,
                    [6.0, 34.0],
                    30.0,
                    Linear2::IDENTITY,
                    hex("#000000", 1.0),
                ),
                slug_glyph(
                    assets.ring,
                    [50.0, 34.0],
                    30.0,
                    Linear2::IDENTITY,
                    hex("#000000", 0.6),
                ),
                slug_glyph(
                    assets.wedge,
                    [6.0, 68.0],
                    30.0,
                    Linear2::IDENTITY,
                    hex("#ffffff", 1.0),
                ),
                slug_glyph(
                    assets.wedge,
                    [50.0, 68.0],
                    30.0,
                    Linear2::IDENTITY,
                    hex("#ffffff", 0.6),
                ),
            ]),
        ],
    )
}

fn atlas_midtone(name: &str) -> Case {
    slug_case(
        name,
        Oklch::TRANSPARENT,
        vec![
            mid(),
            atlas_run(&[
                (
                    rect(4.0, 4.0, 16.0, 16.0),
                    rect(0.0, 0.0, 16.0, 16.0),
                    "#000000",
                    1.0,
                ),
                (
                    rect(24.0, 4.0, 16.0, 16.0),
                    rect(0.0, 0.0, 16.0, 16.0),
                    "#000000",
                    0.6,
                ),
                (
                    rect(44.0, 4.0, 16.0, 16.0),
                    rect(32.0, 0.0, 16.0, 16.0),
                    "#ffffff",
                    1.0,
                ),
                (
                    rect(64.0, 4.0, 16.0, 16.0),
                    rect(32.0, 0.0, 16.0, 16.0),
                    "#ffffff",
                    0.6,
                ),
                (
                    rect(4.0, 30.0, 16.0, 16.0),
                    rect(16.0, 0.0, 16.0, 16.0),
                    "#dd0030",
                    0.8,
                ),
                (
                    rect(24.0, 30.0, 16.0, 16.0),
                    rect(16.0, 0.0, 16.0, 16.0),
                    "#dd0030",
                    0.4,
                ),
            ]),
        ],
    )
}

fn exact_tone(case: Case) -> Case {
    Case {
        plan: FramePlan {
            text_tone: ToneCurve::EXACT,
            ..case.plan
        },
        ..case
    }
}

pub fn cases() -> Vec<Case> {
    let assets = slug_assets();
    let white = hex("#ffffff", 1.0);
    vec![
        slug_case(
            "slug_glyphs_at_three_sizes",
            Oklch::TRANSPARENT,
            vec![
                dark(),
                slug_run(vec![
                    slug_glyph(assets.ring, [4.0, 22.0], 18.0, Linear2::IDENTITY, white),
                    slug_glyph(
                        assets.wedge,
                        [30.0, 40.0],
                        36.0,
                        Linear2::IDENTITY,
                        hex("#ece030", 1.0),
                    ),
                    slug_glyph(
                        assets.blob,
                        [10.0, 66.0],
                        22.0,
                        Linear2::IDENTITY,
                        hex("#20c090", 0.7),
                    ),
                    slug_glyph(assets.ring, [60.0, 62.0], 30.0, Linear2::IDENTITY, white),
                ]),
            ],
        ),
        slug_case(
            "slug_rotated_and_skewed",
            Oklch::TRANSPARENT,
            vec![
                dark(),
                slug_run(vec![
                    slug_glyph(
                        assets.wedge,
                        [24.0, 56.0],
                        40.0,
                        Linear2::rotation(0.5),
                        white,
                    ),
                    slug_glyph(
                        assets.blob,
                        [50.0, 60.0],
                        28.0,
                        Linear2::skew(-0.35, 0.0),
                        hex("#dd7030", 1.0),
                    ),
                    slug_glyph(
                        assets.ring,
                        [70.0, 30.0],
                        24.0,
                        Linear2::rotation(-0.9).then(Linear2::skew(0.0, 0.2)),
                        hex("#4060e0", 1.0),
                    ),
                ]),
            ],
        ),
        slug_case(
            "slug_clipped_in_group",
            Oklch::TRANSPARENT,
            vec![
                dark(),
                PaintCommand::PushClip(rect(10.0, 14.0, 60.0, 40.0)),
                PaintCommand::PushOpacity(0.6),
                slug_run(vec![
                    slug_glyph(assets.ring, [6.0, 50.0], 44.0, Linear2::IDENTITY, white),
                    slug_glyph(
                        assets.wedge,
                        [44.0, 60.0],
                        44.0,
                        Linear2::rotation(0.2),
                        hex("#ece030", 1.0),
                    ),
                ]),
                PaintCommand::PopOpacity,
                PaintCommand::PopClip,
            ],
        ),
        slug_midtone("slug_on_midtone"),
        exact_tone(slug_midtone("slug_exact_tone_on_midtone")),
        atlas_midtone("glyph_atlas_on_midtone"),
        exact_tone(atlas_midtone("glyph_atlas_exact_tone_on_midtone")),
    ]
}
