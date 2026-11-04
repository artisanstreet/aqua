use super::atlas::{AtlasPages, GlyphBitmap, GlyphKey, Tile};
use super::map::{indistinguishable, shimmer_color};
use super::pipeline::{GlyphInstance, MaskDraw, Pipelines};
use super::raster::{atlas_rgba, GlyphRequest, Rasterizer};
use super::RenderingParameters;
use crate::gpui_atlas::{AtlasKey, SharedGpuiAtlas};
use crate::text::{ShapedRun, TextBackend, TextFrame, TextStats};
use aqua_core::scene::TextRasterMode;
use aqua_text::raster::RasterKey;
use aqua_text::{ResolveError, RunId};

struct PendingGlyph {
    line: usize,
    kind: u8,
    page: usize,
    tile: u32,
    view: wgpu::TextureView,
    clip: [u32; 4],
    instance: GlyphInstance,
    subpixel: bool,
}

pub(in crate::text) struct Provider {
    backend: TextBackend,
    pipelines: Pipelines,
    atlas: AtlasPages,
    subpixel_atlas: AtlasPages,
    color_atlas: SharedGpuiAtlas,
    raster: Rasterizer,
    parameters: RenderingParameters,
    mode: TextRasterMode,
}

impl Provider {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        backend: TextBackend,
        color_atlas: SharedGpuiAtlas,
    ) -> Result<Self, String> {
        Ok(Self {
            backend,
            pipelines: Pipelines::new(device, format)?,
            atlas: AtlasPages::new(),
            subpixel_atlas: AtlasPages::new(),
            color_atlas,
            raster: Rasterizer::new(),
            parameters: RenderingParameters::cosmic_from_env()
                .validate()
                .map_err(str::to_owned)?,
            mode: TextRasterMode::PlatformDefault,
        })
    }

    pub fn set_polychrome_atlas(&mut self, atlas: SharedGpuiAtlas) {
        self.color_atlas = atlas;
    }

    pub fn set_mode(&mut self, _device: &wgpu::Device, mode: TextRasterMode) -> Result<(), String> {
        if self.mode != mode {
            self.atlas = AtlasPages::new();
            self.subpixel_atlas = AtlasPages::new();
            self.mode = mode;
        }
        Ok(())
    }

    pub fn encode(
        &mut self,
        frame: TextFrame<'_>,
        runs: &[ShapedRun],
    ) -> Result<TextStats, String> {
        let mut stats = TextStats::default();
        let mut fonts = self.backend.fonts.lock();
        let registry = self.backend.runs.lock();
        for run in runs {
            let handle = match registry.resolve_expected(RunId(run.id), run.revision) {
                Ok(handle) => handle,
                Err(ResolveError::Unknown) => {
                    stats.stale_skipped += 1;
                    continue;
                }
                Err(ResolveError::RevisionMismatch { .. }) => {
                    stats.revision_mismatches += 1;
                    continue;
                }
            };
            let window_subpixel = false;
            let default = run
                .gpui_color
                .map(|color| [color.h, color.s, color.l, color.a]);
            let source_default = run.gpui_source_color.map(|color| {
                [
                    color.hue_degrees,
                    color.saturation,
                    color.lightness,
                    color.alpha,
                ]
            });
            let laid = handle
                .layout
                .laid_out()
                .ok_or("GPUI text requires glyph color geometry")?;
            let scale = handle.layout.scale();
            if !scale.is_finite() || scale <= 0.0 || !run.scale.is_finite() || run.scale <= 0.0 {
                return Err("invalid GPUI glyph scale".into());
            }
            let clip = physical_clip(run.clip_physical, frame.width, frame.height)?;
            let params = if self.mode == TextRasterMode::Outline {
                self.parameters.exact_coverage()
            } else {
                self.parameters
            };
            let mut pending = Vec::new();
            let ordered: Vec<_> = laid
                .lines
                .iter()
                .enumerate()
                .flat_map(|(index, line)| line.glyphs.iter().map(move |glyph| (index, line, glyph)))
                .collect();
            #[cfg(feature = "gpui-diagnostics")]
            let ordered = if let Some(diagnostic) = super::glyph_diagnostic()?
                .filter(|diagnostic| diagnostic.text == handle.layout.display_text())
            {
                let mut selected = Vec::with_capacity(diagnostic.glyph_ids.len());
                for id in diagnostic.glyph_ids {
                    let mut matches = ordered
                        .iter()
                        .copied()
                        .filter(|(_, _, glyph)| glyph.glyph_id == id);
                    let item = matches.next().ok_or("GPUI diagnostic glyph missing")?;
                    if matches.next().is_some() {
                        return Err("GPUI diagnostic glyph is ambiguous".into());
                    }
                    selected.push(item);
                }
                selected
            } else {
                ordered
            };
            for (line_index, shaped_line, shaped) in ordered {
                let glyph = shaped;
                if glyph.glyph_id == 3
                    && Rasterizer::is_emoji_font(
                        &mut fonts,
                        glyph.font_id,
                        glyph.font_weight,
                        glyph.font_stretch,
                    )?
                {
                    continue;
                }
                let is_color = self.raster.is_color_glyph(
                    &mut fonts,
                    glyph.font_id,
                    glyph.font_weight,
                    glyph.font_stretch,
                    glyph.glyph_id,
                )?;
                #[cfg(feature = "gpui-diagnostics")]
                if super::glyph_diagnostic()?
                    .is_some_and(|diagnostic| diagnostic.text == handle.layout.display_text())
                    && is_color == (glyph.glyph_id == 117)
                {
                    return Err("GPUI diagnostic glyph color classification mismatch".into());
                }
                let x = run.at_physical[0] + (shaped.x + shaped.x_offset) * run.scale;
                let y = run.at_physical[1]
                    + (shaped_line.baseline_y + shaped.y - shaped.y_offset) * run.scale;
                let qx = if is_color {
                    round_half_toward_zero(x)
                } else {
                    round_half_toward_zero(x * 4.0) / 4.0
                };
                let qy = round_half_toward_zero(y);
                let variant = (qx.fract() * 4.0) as u8;
                let font_size = glyph.font_size_px / handle.layout.scale();
                let cache = RasterKey {
                    font_id: glyph.font_id,
                    weight: glyph.font_weight,
                    stretch: glyph.font_stretch,
                    synthetic_italic: glyph.synthetic_italic,
                    glyph_id: glyph.glyph_id,
                    size_bits: glyph.font_size_px.to_bits(),
                    subpixel: variant,
                };
                let subpixel = !is_color && window_subpixel;
                let key = GlyphKey {
                    cache,
                    subpixel,
                    emoji: is_color,
                    scale_bits: scale.to_bits(),
                    font_size_bits: font_size.to_bits(),
                    x_variant: variant,
                    hint: is_color || self.mode != TextRasterMode::Outline,
                };
                let (page, tile, view, [width, height]) = if is_color {
                    let atlas_key = AtlasKey::Glyph(key);
                    let cached = self
                        .color_atlas
                        .lock()
                        .map_err(|_| "GPUI atlas lock poisoned")?
                        .get(atlas_key);
                    let placement = match cached {
                        Some(placement) => placement,
                        None => {
                            let request = GlyphRequest {
                                font_id: glyph.font_id,
                                weight: glyph.font_weight,
                                stretch: glyph.font_stretch,
                                synthetic_italic: glyph.synthetic_italic,
                                glyph_id: glyph.glyph_id,
                                font_size,
                                scale,
                                x_variant: variant,
                                is_emoji: true,
                                subpixel: false,
                                hint: true,
                            };
                            let Some(image) = self.raster.rasterize(&mut fonts, &request)? else {
                                continue;
                            };
                            if image.placement.width == 0 || image.placement.height == 0 {
                                continue;
                            }
                            let bgra = atlas_rgba(&image, false)?;
                            self.color_atlas
                                .lock()
                                .map_err(|_| "GPUI atlas lock poisoned")?
                                .insert(
                                    frame.device,
                                    frame.queue,
                                    atlas_key,
                                    [image.placement.width, image.placement.height],
                                    [image.placement.left, image.placement.top],
                                    &bgra,
                                )?
                        }
                    };
                    let [x, y, width, height] = placement.tile;
                    (
                        placement.page_index,
                        Tile {
                            id: placement.tile_id,
                            x,
                            y,
                            width,
                            height,
                            left: placement.bearings[0],
                            top: placement.bearings[1],
                        },
                        placement.view,
                        placement.page_size,
                    )
                } else {
                    let atlas = if subpixel {
                        &mut self.subpixel_atlas
                    } else {
                        &mut self.atlas
                    };
                    let (page, tile) = match atlas.get(&key) {
                        Some(tile) => tile,
                        None => {
                            let request = GlyphRequest {
                                font_id: glyph.font_id,
                                weight: glyph.font_weight,
                                stretch: glyph.font_stretch,
                                synthetic_italic: glyph.synthetic_italic,
                                glyph_id: glyph.glyph_id,
                                font_size,
                                scale,
                                x_variant: variant,
                                is_emoji: false,
                                subpixel,
                                hint: self.mode != TextRasterMode::Outline,
                            };
                            let Some(image) = self.raster.rasterize(&mut fonts, &request)? else {
                                continue;
                            };
                            if image.placement.width == 0 || image.placement.height == 0 {
                                continue;
                            }
                            let rgba = atlas_rgba(&image, subpixel)?;
                            atlas.insert(
                                frame.device,
                                frame.queue,
                                key,
                                GlyphBitmap {
                                    width: image.placement.width,
                                    height: image.placement.height,
                                    left: image.placement.left,
                                    top: image.placement.top,
                                    rgba: &rgba,
                                },
                            )?
                        }
                    };
                    (page, tile, atlas.view(page).clone(), atlas.size(page))
                };
                #[cfg(feature = "gpui-diagnostics")]
                if super::glyph_diagnostic()?
                    .is_some_and(|diagnostic| diagnostic.text == handle.layout.display_text())
                {
                    super::GLYPH_DIAGNOSTIC_DRAWS
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    println!(
                        "gpui_glyph_diagnostic id={} color={} font={:?} origin=({qx},{qy}) tile=({},{},{},{}) page=({width},{height}) bearings=({},{})",
                        glyph.glyph_id,
                        is_color,
                        glyph.font_id,
                        tile.x,
                        tile.y,
                        tile.width,
                        tile.height,
                        tile.left,
                        tile.top
                    );
                }
                let range_opacity = run
                    .gpui_text_opacity
                    .as_ref()
                    .and_then(|ranges| {
                        ranges
                            .iter()
                            .find(|range| range.bytes.contains(&shaped.src_byte_start))
                    })
                    .map_or(1.0, |range| range.opacity);
                let color = if is_color {
                    [0.0, 0.0, 0.0, run.gpui_opacity]
                } else if run.gpui_text_color_map.is_some() {
                    [0.0; 4]
                } else {
                    let mut color = shaped
                        .source_color_hsla
                        .map(source_to_gpu)
                        .or(shaped.color_hsla)
                        .or_else(|| source_default.map(source_to_gpu))
                        .or(default)
                        .ok_or("GPUI glyph requires source float HSLA")?;
                    color[3] *= run.gpui_opacity;
                    color[3] *= range_opacity;
                    color
                };
                let instance = GlyphInstance {
                    bounds: [
                        qx.trunc() + tile.left as f32 * run.scale,
                        qy.trunc() - tile.top as f32 * run.scale,
                        tile.width as f32 * run.scale,
                        tile.height as f32 * run.scale,
                    ],
                    uv_bounds: [
                        tile.x as f32 / width as f32,
                        tile.y as f32 / height as f32,
                        tile.width as f32 / width as f32,
                        tile.height as f32 / height as f32,
                    ],
                    hsla: color,
                };
                let mut spans = Vec::new();
                if let Some(map) = run.gpui_text_color_map.filter(|_| !is_color) {
                    if !run.gpui_window_scale.is_finite() || run.gpui_window_scale <= 0.0 {
                        return Err("invalid GPUI window scale".into());
                    }
                    let source = match (shaped.source_color_hsla, shaped.color_hsla) {
                        (Some(source), _) => Some(source),
                        (None, Some(_)) => None,
                        (None, None) => source_default,
                    }
                    .ok_or("GPUI mapped glyph requires source Palette color")?;
                    let target = [
                        map.target.hue_degrees,
                        map.target.saturation,
                        map.target.lightness,
                        map.target.alpha,
                    ];
                    let left = (instance.bounds[0] as i64).max(i64::from(clip[0]));
                    let right = ((instance.bounds[0] + instance.bounds[2]) as i64)
                        .min(i64::from(clip[0] + clip[2]));
                    for x in left..right {
                        let position = if map.bounds.w <= 0.0 {
                            f32::NAN
                        } else {
                            (((x as f32 + 0.5) / run.gpui_window_scale) - map.bounds.x)
                                / map.bounds.w
                        };
                        let mapped = shimmer_color(source, target, position, map.phase, map.spread);
                        if let Some((_, end, previous)) = spans.last_mut() {
                            if indistinguishable(*previous, mapped) {
                                *end += 1;
                                continue;
                            }
                        }
                        spans.push((x, x + 1, mapped));
                    }
                } else {
                    spans.push((i64::from(clip[0]), i64::from(clip[0] + clip[2]), [0.0; 4]));
                }
                for (left, right, source_color) in spans {
                    let mut instance = instance;
                    if run.gpui_text_color_map.is_some() && !is_color {
                        instance.hsla = source_to_gpu(source_color);
                        instance.hsla[3] *= run.gpui_opacity;
                        instance.hsla[3] *= range_opacity;
                    }
                    pending.push(PendingGlyph {
                        line: line_index,
                        kind: if is_color {
                            2
                        } else if subpixel {
                            1
                        } else {
                            0
                        },
                        page,
                        tile: tile.id,
                        view: view.clone(),
                        clip: [left as u32, clip[1], (right - left) as u32, clip[3]],
                        instance,
                        subpixel,
                    });
                }
            }
            pending.sort_by_key(|glyph| (glyph.line, glyph.kind, glyph.page, glyph.tile));
            for glyph in &pending {
                let draw = MaskDraw {
                    device: frame.device,
                    encoder: &mut *frame.encoder,
                    target: frame.target,
                    atlas: &glyph.view,
                    size: [frame.width, frame.height],
                    clip: glyph.clip,
                    params,
                    instances: std::slice::from_ref(&glyph.instance),
                    subpixel: glyph.subpixel,
                };
                if glyph.kind == 2 {
                    self.pipelines.encode_color(draw)?;
                } else {
                    self.pipelines.encode(draw)?;
                }
            }
            stats.areas += 1;
        }
        Ok(stats)
    }
}

fn round_half_toward_zero(value: f32) -> f32 {
    (value.abs() - 0.5).ceil().copysign(value)
}

fn physical_clip(clip: Option<[f32; 4]>, width: u32, height: u32) -> Result<[u32; 4], String> {
    let Some([x, y, w, h]) = clip else {
        return Ok([0, 0, width, height]);
    };
    if ![x, y, w, h].into_iter().all(f32::is_finite) {
        return Err("invalid GPUI text clip".into());
    }
    let left = x.floor().clamp(0.0, width as f32) as u32;
    let top = y.floor().clamp(0.0, height as f32) as u32;
    let right = (x + w.max(0.0)).ceil().clamp(0.0, width as f32) as u32;
    let bottom = (y + h.max(0.0)).ceil().clamp(0.0, height as f32) as u32;
    Ok([
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    ])
}

fn source_to_gpu(color: [f32; 4]) -> [f32; 4] {
    let hue = palette::RgbHue::from_degrees(color[0]);
    [
        hue.into_positive_degrees() / 360.0,
        color[1],
        color[2],
        color[3],
    ]
}
