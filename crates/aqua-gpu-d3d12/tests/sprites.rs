#![cfg(windows)]

mod support;

use std::sync::Arc;

use aqua_core::geometry::Rect;
use aqua_core::vector::{Brush, Drawing, GradientStop, PaintedPath, PathCommand, VectorSource};
use aqua_core::{Oklch, Point};
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend};
use aqua_gpu_ref::{conformance, RefBackend};
use aqua_graph::{Extent, FramePlan, PlanLimits, RenderGraph, TextureRef};
use aqua_media::atlas::{
    AtlasConfig, GlyphBitmap, GlyphKey, ImageKey, PageFormat, Placement, Slot, StreamBudget,
    SUBPIXEL_BINS,
};
use aqua_media::residency::{Residency, ResidencyConfig, VectorMode};
use aqua_media::upload::{glyph_instance, glyph_passes, image_instance, image_pass, upload_batch};
use aqua_protocol::{GpuBackend, SurfacePixels, UploadBatch};

struct Rig {
    label: String,
    reference: RefBackend,
    gpu: OffscreenBackend,
}

struct Diff {
    max: u8,
    over: usize,
    worst: (usize, usize),
}

fn rigs() -> Vec<Rig> {
    [AdapterPreference::Warp, AdapterPreference::HighPerformance]
        .into_iter()
        .filter_map(|preference| match OffscreenBackend::new(preference) {
            Ok(gpu) => Some(Rig {
                label: format!("{preference:?} software={}", gpu.is_software()),
                reference: RefBackend::new(),
                gpu,
            }),
            Err(error) => {
                eprintln!("SKIP {preference:?}: {error}");
                None
            }
        })
        .collect()
}

impl Rig {
    fn upload(&mut self, batch: &UploadBatch) {
        self.reference.upload(batch.clone()).unwrap();
        self.gpu.upload(batch.clone()).unwrap();
    }

    fn render(&mut self, plan: &FramePlan) -> (SurfacePixels, SurfacePixels) {
        let expected = conformance::render(&mut self.reference, plan).unwrap();
        let actual = conformance::render(&mut self.gpu, plan).unwrap();
        (expected, actual)
    }
}

fn diff(expected: &SurfacePixels, actual: &SurfacePixels, tolerance: u8) -> Diff {
    assert_eq!(expected.extent, actual.extent);
    let width = expected.extent.width as usize;
    let mut result = Diff {
        max: 0,
        over: 0,
        worst: (0, 0),
    };
    for (index, (a, b)) in expected.rgba8.iter().zip(&actual.rgba8).enumerate() {
        let delta = a.abs_diff(*b);
        if delta > tolerance {
            result.over += 1;
        }
        if delta > result.max {
            result.max = delta;
            result.worst = ((index / 4) % width, (index / 4) / width);
        }
    }
    result
}

fn check(label: &str, case: &str, rig: &mut Rig, plan: &FramePlan, tolerance: u8) -> Diff {
    let (expected, actual) = rig.render(plan);
    let result = diff(&expected, &actual, tolerance);
    eprintln!(
        "{label} {case}: max {} over-tolerance {} worst at {:?}",
        result.max, result.over, result.worst
    );
    assert_eq!(
        result.over, 0,
        "{label} {case}: {} samples differ by more than {tolerance}, worst {:?}",
        result.over, result.worst
    );
    let lit = actual
        .rgba8
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] > 0)
        .count();
    assert!(lit > 0, "{label} {case}: the frame is empty");
    result
}

fn surface_graph(width: u32, height: u32) -> RenderGraph {
    RenderGraph::new(
        Extent { width, height },
        Oklch::from_hex("#20242c").unwrap(),
    )
}

fn plan(graph: RenderGraph) -> FramePlan {
    graph.plan(PlanLimits::default()).unwrap()
}

fn color(hex: &str, alpha: f32) -> Oklch {
    Oklch::from_hex(hex).unwrap().with_alpha(alpha)
}

fn glyph_bitmap(key: &GlyphKey) -> Option<GlyphBitmap> {
    let size = (key.size_sixty_fourths / 64) as i32;
    let width = size as u32 - 2 + key.glyph % 5;
    let height = size as u32;
    let shift = key.subpixel as f32 / SUBPIXEL_BINS as f32;
    let center = (width as f32 * 0.5 + shift, height as f32 * 0.5);
    let radius = height as f32 * 0.38;
    let thickness = 1.0 + (key.glyph % 3) as f32;
    let mut coverage = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let (px, py) = (x as f32 + 0.5 - center.0, y as f32 + 0.5 - center.1);
            let ring = ((px * px + py * py).sqrt() - radius).abs();
            let bar = (px - py * 0.3).abs();
            let value = match key.glyph % 2 {
                0 => (thickness * 0.5 + 0.5 - ring).clamp(0.0, 1.0),
                _ => (thickness * 0.5 + 0.5 - bar).clamp(0.0, 1.0),
            };
            coverage.push((value * 255.0).round() as u8);
        }
    }
    Some(GlyphBitmap {
        width,
        height,
        left: 0,
        top: size - 3,
        coverage,
    })
}

fn glyph_key(glyph: u32, size: u32, subpixel: u8) -> GlyphKey {
    GlyphKey {
        font: 1,
        glyph,
        size_sixty_fourths: size * 64,
        subpixel,
        variant: 0,
    }
}

fn resident(slot: Slot) -> Placement {
    match slot {
        Slot::Resident(placement) => placement,
        Slot::Blank { .. } => panic!("expected a resident glyph"),
    }
}

fn gradient_image(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let (u, v) = (
                x as f32 / (width - 1) as f32,
                y as f32 / (height - 1) as f32,
            );
            let inside = {
                let (dx, dy) = (u - 0.5, v - 0.5);
                ((0.5 - (dx * dx + dy * dy).sqrt() * 1.1) * width as f32).clamp(0.0, 1.0)
            };
            let channels = [u * 0.9, v * 0.9, 0.3 + 0.5 * (1.0 - u)];
            pixels.extend(channels.map(|channel| (channel * inside * 255.0).round() as u8));
            pixels.push((inside * 255.0).round() as u8);
        }
    }
    pixels
}

fn star_drawing() -> VectorSource {
    let point = |angle: f32, radius: f32| {
        Point::new(16.0 + radius * angle.cos(), 16.0 + radius * angle.sin())
    };
    let mut commands = Vec::new();
    for index in 0..10 {
        let angle = -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.0;
        let target = point(angle, if index % 2 == 0 { 14.0 } else { 6.0 });
        commands.push(if index == 0 {
            PathCommand::Move(target)
        } else {
            PathCommand::Line(target)
        });
    }
    commands.push(PathCommand::Close);
    let brush = Brush::Linear {
        start: Point::new(0.0, 0.0),
        end: Point::new(32.0, 32.0),
        stops: Arc::from(vec![
            GradientStop {
                position: 0.0,
                color: color("#ff7a30", 1.0),
            },
            GradientStop {
                position: 1.0,
                color: color("#3060ff", 1.0),
            },
        ]),
    };
    VectorSource::Drawing(Arc::new(Drawing {
        view_box: Rect {
            x: 0.0,
            y: 0.0,
            w: 32.0,
            h: 32.0,
        },
        paths: vec![PaintedPath::fill(commands, brush)],
    }))
}

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" fill="none" stroke="#e0a030" stroke-width="3"/><path d="M7 12 L11 16 L17 8" fill="none" stroke="#30c0a0" stroke-width="2.5" stroke-linecap="round"/></svg>"##;

#[test]
fn glyph_runs_from_the_residency_match_the_reference() {
    let _software = support::software();
    for mut rig in rigs() {
        let mut residency = Residency::new(ResidencyConfig::default());
        residency.begin_frame();
        let mut rasterize = |key: &GlyphKey| glyph_bitmap(key);
        let mut instances = Vec::new();
        let palette = [
            ("#f2f4f8", 1.0),
            ("#5aa0ff", 0.85),
            ("#ffb454", 1.0),
            ("#7fe0a0", 0.6),
        ];
        for row in 0..6u32 {
            for column in 0..12u32 {
                let glyph = row * 12 + column;
                let pen_x = 6.0 + column as f32 * 16.3 + (glyph % 4) as f32 * 0.25;
                let (floor, bin) = aqua_media::atlas::subpixel_bin(pen_x);
                let size = [12, 14, 16][(row % 3) as usize];
                let slot = residency
                    .glyph(glyph_key(glyph, size, bin), &mut rasterize)
                    .unwrap();
                let (name, alpha) = palette[(glyph % 4) as usize];
                let clip = (row == 4).then_some(Rect {
                    x: 0.0,
                    y: 70.0,
                    w: 100.0,
                    h: 22.0,
                });
                let opacity = if row == 5 { 0.5 } else { 1.0 };
                instances.push((
                    resident(slot).page,
                    glyph_instance(
                        &resident(slot),
                        [floor, 20 + row as i32 * 22],
                        color(name, alpha),
                        clip,
                        opacity,
                    ),
                ));
            }
        }
        rig.upload(&upload_batch(residency.finish_frame()));
        let mut graph = surface_graph(208, 140);
        glyph_passes(instances, TextureRef::Surface)
            .into_iter()
            .for_each(|pass| graph.push(pass));
        let label = rig.label.clone();
        check(&label, "glyph grid", &mut rig, &plan(graph), 2);
    }
}

#[test]
fn images_sampled_at_every_scale_match_the_reference() {
    let _software = support::software();
    for mut rig in rigs() {
        let mut residency = Residency::new(ResidencyConfig::default());
        residency.begin_frame();
        let key = ImageKey {
            asset: 1,
            frame: 0,
            width: 24,
            height: 24,
        };
        let slot = residency.image(key, &gradient_image(24, 24)).unwrap();
        let placement = resident(slot);
        rig.upload(&upload_batch(residency.finish_frame()));
        let rect = |x: f32, y: f32, w: f32, h: f32| Rect { x, y, w, h };
        let image = |bounds: Rect, radius: f32, opacity: f32, clip: Option<Rect>| {
            image_instance(&placement, bounds, radius, opacity, clip)
        };
        let instances = vec![
            image(rect(4.0, 4.0, 24.0, 24.0), 0.0, 1.0, None),
            image(rect(36.5, 4.25, 24.0, 24.0), 0.0, 1.0, None),
            image(rect(70.0, 4.0, 60.0, 60.0), 0.0, 1.0, None),
            image(rect(140.0, 4.0, 11.0, 11.0), 0.0, 1.0, None),
            image(rect(4.0, 40.0, 48.0, 48.0), 14.0, 0.7, None),
            image(
                rect(140.0, 30.0, 48.0, 48.0),
                0.0,
                1.0,
                Some(rect(150.0, 40.0, 24.0, 24.0)),
            ),
            image(rect(4.0, 100.0, 90.0, 20.0), 6.0, 1.0, None),
        ];
        let mut graph = surface_graph(200, 130);
        graph.push(image_pass(instances, TextureRef::Surface));
        let label = rig.label.clone();
        check(&label, "image scales", &mut rig, &plan(graph), 4);
    }
}

#[test]
fn vector_paths_and_svg_through_the_atlas_match_the_reference() {
    let _software = support::software();
    for mut rig in rigs() {
        let mut residency = Residency::new(ResidencyConfig::default());
        residency.begin_frame();
        let star = star_drawing();
        let svg = VectorSource::svg(SVG);
        let mask_slot = resident(residency.vector(&star, 32, 32, VectorMode::Mask).unwrap());
        let star_slot = resident(residency.vector(&star, 64, 64, VectorMode::Color).unwrap());
        let svg_slot = resident(residency.vector(&svg, 48, 48, VectorMode::Color).unwrap());
        rig.upload(&upload_batch(residency.finish_frame()));
        let mut graph = surface_graph(180, 90);
        let tinted = glyph_instance(
            &Placement {
                bearing: [0, 0],
                ..mask_slot
            },
            [8, 40],
            color("#ffd060", 1.0),
            None,
            1.0,
        );
        let mut tinted_instance = tinted;
        tinted_instance.bounds = Rect {
            x: 8.0,
            y: 8.0,
            w: 32.0,
            h: 32.0,
        };
        glyph_passes([(mask_slot.page, tinted_instance)], TextureRef::Surface)
            .into_iter()
            .for_each(|pass| graph.push(pass));
        let rect = |x: f32, y: f32, w: f32, h: f32| Rect { x, y, w, h };
        graph.push(image_pass(
            vec![
                image_instance(&star_slot, rect(50.0, 8.0, 64.0, 64.0), 0.0, 1.0, None),
                image_instance(&svg_slot, rect(122.0, 8.0, 48.0, 48.0), 0.0, 1.0, None),
            ],
            TextureRef::Surface,
        ));
        let label = rig.label.clone();
        check(&label, "vectors", &mut rig, &plan(graph), 4);
    }
}

#[test]
fn atlas_eviction_and_page_reuse_keep_every_frame_identical_to_the_reference() {
    let _software = support::software();
    for mut rig in rigs() {
        let small = AtlasConfig {
            format: PageFormat::Mask8,
            base_edge: 64,
            max_edge: 256,
            max_shared_pages: 2,
            budget_bytes: 64 * 64 + 128 * 128,
        };
        let mut residency = Residency::new(ResidencyConfig {
            masks: small,
            ..ResidencyConfig::default()
        });
        let mut rasterize = |key: &GlyphKey| glyph_bitmap(key);
        let mut created = 0;
        let mut released = 0;
        for frame in 0..24u32 {
            residency.begin_frame();
            let mut instances = Vec::new();
            for index in 0..60u32 {
                let glyph = frame * 17 + index * 3;
                let slot = residency
                    .glyph(glyph_key(glyph, 14, (glyph % 4) as u8), &mut rasterize)
                    .unwrap();
                let placement = resident(slot);
                instances.push((
                    placement.page,
                    glyph_instance(
                        &placement,
                        [4 + (index % 12) as i32 * 16, 18 + (index / 12) as i32 * 18],
                        color("#f2f4f8", 1.0),
                        None,
                        1.0,
                    ),
                ));
            }
            let updates = residency.finish_frame();
            created += updates.created.len();
            released += updates.released.len();
            rig.upload(&upload_batch(updates));
            let mut graph = surface_graph(200, 100);
            glyph_passes(instances, TextureRef::Surface)
                .into_iter()
                .for_each(|pass| graph.push(pass));
            let label = rig.label.clone();
            check(
                &label,
                &format!("eviction frame {frame}"),
                &mut rig,
                &plan(graph),
                2,
            );
        }
        let stats = residency.stats().masks;
        eprintln!(
            "{} atlas: pages {} bytes {} evictions {} created {created} released {released} over-budget {}",
            rig.label,
            stats.pages,
            stats.resident_bytes,
            stats.page_evictions,
            stats.over_budget_pages
        );
        assert!(
            stats.page_evictions > 0,
            "the scenario must force page eviction"
        );
        assert!(created > 2);
    }
}

#[test]
fn streamed_images_render_identically_while_partial_and_when_complete() {
    let _software = support::software();
    for mut rig in rigs() {
        let config = ResidencyConfig {
            stream: StreamBudget {
                bytes_per_frame: 128 * 1024,
            },
            ..ResidencyConfig::default()
        };
        let mut residency = Residency::new(config);
        let key = ImageKey {
            asset: 5,
            frame: 0,
            width: 256,
            height: 256,
        };
        residency.begin_frame();
        let placement = resident(residency.image(key, &gradient_image(256, 256)).unwrap());
        let mut frames = 0;
        loop {
            frames += 1;
            let updates = residency.finish_frame();
            rig.upload(&upload_batch(updates));
            let mut graph = surface_graph(140, 140);
            graph.push(image_pass(
                vec![image_instance(
                    &placement,
                    Rect {
                        x: 6.0,
                        y: 6.0,
                        w: 128.0,
                        h: 128.0,
                    },
                    12.0,
                    1.0,
                    None,
                )],
                TextureRef::Surface,
            ));
            let label = rig.label.clone();
            let complete = !residency.page_pending(placement.page);
            check(
                &label,
                &format!("streamed frame {frames} complete={complete}"),
                &mut rig,
                &plan(graph),
                4,
            );
            if complete {
                break;
            }
            residency.begin_frame();
            residency.image_resident(&key);
        }
        assert_eq!(
            frames, 2,
            "256 KiB at 128 KiB per frame streams in two frames"
        );
    }
}

#[test]
fn released_pages_free_gpu_memory_and_recycled_indices_render_correctly() {
    let _software = support::software();
    for mut rig in rigs() {
        let masks = AtlasConfig {
            format: PageFormat::Mask8,
            base_edge: 64,
            max_edge: 256,
            max_shared_pages: 4,
            budget_bytes: 1 << 20,
        };
        let mut residency = Residency::new(ResidencyConfig {
            masks,
            ..ResidencyConfig::default()
        });
        let mut rasterize = |key: &GlyphKey| glyph_bitmap(key);
        let label = rig.label.clone();
        let mut draw =
            |rig: &mut Rig, residency: &mut Residency, glyphs: std::ops::Range<u32>, name: &str| {
                residency.begin_frame();
                let mut instances = Vec::new();
                for (index, glyph) in glyphs.enumerate() {
                    let placement = resident(
                        residency
                            .glyph(glyph_key(glyph, 14, 0), &mut rasterize)
                            .unwrap(),
                    );
                    let pen = [4 + (index as i32 % 12) * 16, 18 + (index as i32 / 12) * 18];
                    instances.push((
                        placement.page,
                        glyph_instance(&placement, pen, color("#f2f4f8", 1.0), None, 1.0),
                    ));
                }
                let trimmed = residency.trim_idle(1);
                rig.upload(&upload_batch(residency.finish_frame()));
                let mut graph = surface_graph(200, 100);
                glyph_passes(instances, TextureRef::Surface)
                    .into_iter()
                    .for_each(|pass| graph.push(pass));
                check(&label, name, rig, &plan(graph), 2);
                trimmed
            };
        draw(&mut rig, &mut residency, 0..70, "wide set");
        let pages_wide = residency.stats().masks.pages;
        draw(&mut rig, &mut residency, 0..4, "narrow set");
        let trimmed = draw(&mut rig, &mut residency, 0..4, "narrow set again");
        assert!(trimmed >= 1, "idle pages must be released");
        draw(&mut rig, &mut residency, 100..170, "new wide set");
        rig.gpu.persistent_stats();
        let stats = residency.stats().masks;
        let gpu = rig.gpu.persistent_stats();
        eprintln!(
            "{label} pages wide {pages_wide} now {} resident {} gpu textures {} gpu bytes {}",
            stats.pages, stats.resident_bytes, gpu.textures, gpu.resident_bytes
        );
        assert_eq!(gpu.resident_bytes, stats.resident_bytes);
        assert_eq!(gpu.textures, stats.pages);
    }
}

#[test]
fn dedicated_image_pages_are_released_and_replaced_without_corrupting_frames_in_flight() {
    let _software = support::software();
    for mut rig in rigs() {
        let mut residency = Residency::new(ResidencyConfig::default());
        let label = rig.label.clone();
        for generation in 0..6u32 {
            residency.begin_frame();
            let key = ImageKey {
                asset: 9,
                frame: generation,
                width: 1100,
                height: 40,
            };
            let mut pixels = gradient_image(1100, 40);
            pixels
                .iter_mut()
                .step_by(4)
                .for_each(|red| *red = red.wrapping_add((generation * 40) as u8));
            let placement = resident(residency.image(key, &pixels).unwrap());
            assert_eq!(placement.page_width, 1100);
            rig.upload(&upload_batch(residency.finish_frame()));
            let mut graph = surface_graph(220, 20);
            graph.push(image_pass(
                vec![image_instance(
                    &placement,
                    Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 220.0,
                        h: 20.0,
                    },
                    0.0,
                    1.0,
                    None,
                )],
                TextureRef::Surface,
            ));
            check(
                &label,
                &format!("dedicated generation {generation}"),
                &mut rig,
                &plan(graph),
                4,
            );
            residency.begin_frame();
            residency.remove_image(&key);
            rig.upload(&upload_batch(residency.finish_frame()));
        }
        let (_, _) = rig.render(&plan(surface_graph(8, 8)));
        assert_eq!(rig.gpu.persistent_stats().textures, 0);
        assert_eq!(rig.gpu.persistent_stats().resident_bytes, 0);
    }
}
