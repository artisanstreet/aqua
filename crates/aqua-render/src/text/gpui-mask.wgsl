// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

enable dual_source_blending;
enable clip_distances;

struct Parameters {
    gamma: vec4<f32>,
    grayscale_contrast: f32,
    subpixel_contrast: f32,
    bgr: u32,
    unused: u32,
}

struct Input {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) bounds: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) bounds: vec4<f32>,
    @builtin(clip_distances) clip_distances: array<f32, 4>,
}

struct SubpixelOutput {
    @location(0) @blend_src(0) foreground: vec4<f32>,
    @location(0) @blend_src(1) coverage: vec4<f32>,
}

@group(0) @binding(0) var glyph_texture: texture_2d<f32>;
@group(0) @binding(1) var glyph_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Parameters;

fn corrected(a: vec3<f32>, b: vec3<f32>, gamma: vec4<f32>) -> vec3<f32> {
    let adjustment = gamma.x * b + gamma.y;
    let correction = adjustment * a + (gamma.z * b + gamma.w);
    return a + a * (1.0 - a) * correction;
}

fn contrast(sample: vec3<f32>, color: vec3<f32>, factor: f32) -> vec3<f32> {
    let brightness = dot(color, vec3<f32>(0.30, 0.59, 0.11));
    let k = factor * clamp(4.0 * (0.75 - brightness), 0.0, 1.0);
    return sample * (k + 1.0) / (sample * k + 1.0);
}

@fragment
fn grayscale(input: Input) -> @location(0) vec4<f32> {
    let sample = textureSample(glyph_texture, glyph_sampler, input.uv).r;
    let brightness = dot(input.color.rgb, vec3<f32>(0.30, 0.59, 0.11));
    let enhanced = contrast(vec3<f32>(sample), input.color.rgb, params.grayscale_contrast);
    let alpha = corrected(enhanced, vec3<f32>(brightness), params.gamma).r;
    return vec4<f32>(input.color.rgb, input.color.a * alpha);
}

@fragment
fn polychrome(input: Input) -> @location(0) vec4<f32> {
    let sample = textureSample(glyph_texture, glyph_sampler, input.uv);
    let half_size = input.bounds.zw / 2.0;
    let center = input.bounds.xy + half_size;
    let corner = abs(input.position.xy - center) - half_size;
    let distance = max(corner.x, corner.y);
    let coverage = clamp(0.5 - distance, 0.0, 1.0);
    return vec4<f32>(sample.rgb, sample.a * (input.color.a * coverage));
}

@fragment
fn subpixel(input: Input) -> SubpixelOutput {
    var sample = textureSample(glyph_texture, glyph_sampler, input.uv).rgb;
    if (params.bgr != 0u) {
        sample = sample.bgr;
    }
    let enhanced = contrast(sample, input.color.rgb, params.subpixel_contrast);
    let coverage = input.color.a * corrected(enhanced, input.color.rgb, params.gamma);
    return SubpixelOutput(vec4<f32>(input.color.rgb, 1.0), vec4<f32>(coverage, max(coverage.r, max(coverage.g, coverage.b))));
}

struct Viewport {
    size: vec2<f32>,
    padding: vec2<f32>,
    clip: vec4<f32>,
}

struct GlyphInstance {
    @location(0) bounds: vec4<f32>,
    @location(1) uv_bounds: vec4<f32>,
    @location(2) hsla: vec4<f32>,
}

@group(1) @binding(0) var<uniform> viewport: Viewport;

fn hsla_to_srgba(hsla: vec4<f32>) -> vec4<f32> {
    let h = hsla.x * 6.0;
    let c = (1.0 - abs(2.0 * hsla.z - 1.0)) * hsla.y;
    let x = c * (1.0 - abs((h % 2.0) - 1.0));
    let m = hsla.z - c / 2.0;
    var rgb = vec3<f32>(0.0);
    if (h >= 0.0 && h < 1.0) { rgb = vec3<f32>(c, x, 0.0); }
    else if (h >= 1.0 && h < 2.0) { rgb = vec3<f32>(x, c, 0.0); }
    else if (h >= 2.0 && h < 3.0) { rgb = vec3<f32>(0.0, c, x); }
    else if (h >= 3.0 && h < 4.0) { rgb = vec3<f32>(0.0, x, c); }
    else if (h >= 4.0 && h < 5.0) { rgb = vec3<f32>(x, 0.0, c); }
    else { rgb = vec3<f32>(c, 0.0, x); }
    return vec4<f32>(rgb + vec3<f32>(m), hsla.w);
}

@vertex
fn vertex(@builtin(vertex_index) vertex_id: u32, glyph: GlyphInstance) -> VertexOutput {
    let unit = vec2<f32>(f32(vertex_id & 1u), f32((vertex_id >> 1u) & 1u));
    let pixel = glyph.bounds.xy + unit * glyph.bounds.zw;
    let ndc = pixel / viewport.size * 2.0 - vec2<f32>(1.0);
    let tl = pixel - viewport.clip.xy;
    let br = viewport.clip.xy + viewport.clip.zw - pixel;
    var result: VertexOutput;
    result.position = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    result.uv = glyph.uv_bounds.xy + unit * glyph.uv_bounds.zw;
    result.color = hsla_to_srgba(glyph.hsla);
    result.bounds = glyph.bounds;
    result.clip_distances = array<f32, 4>(tl.x, br.x, tl.y, br.y);
    return result;
}
