struct SvgParameters {
    bounds: vec4<f32>,
    hsla: vec4<f32>,
    gamma: vec4<f32>,
    misc: vec4<f32>,
    matrix: vec4<f32>,
    center_translation: vec4<f32>,
}
@group(1) @binding(0) var svg_texture: texture_2d<f32>;
@group(1) @binding(1) var svg_sampler: sampler;
@group(1) @binding(2) var<uniform> svg: SvgParameters;

struct SvgOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn svg_vertex(@builtin(vertex_index) index: u32) -> SvgOutput {
    let unit = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    let original = svg.bounds.xy + unit * svg.bounds.zw;
    let relative = original - svg.center_translation.xy;
    let point = svg.center_translation.xy + svg.center_translation.zw + vec2<f32>(
        svg.matrix.x * relative.x + svg.matrix.y * relative.y,
        svg.matrix.z * relative.x + svg.matrix.w * relative.y,
    );
    var output: SvgOutput;
    output.position = vec4<f32>(point.x / u.viewport.x * 2.0 - 1.0, 1.0 - point.y / u.viewport.y * 2.0, 0.0, 1.0);
    output.uv = unit;
    return output;
}

@fragment
fn svg_fragment(input: SvgOutput) -> @location(0) vec4<f32> {
    let point = input.position.xy;
    let legacy_aa = max(fwidth(point.x), fwidth(point.y)) * 1.5 + 1e-4;
    let sample = textureSample(svg_texture, svg_sampler, input.uv).r;
    let color = gpui_hsla(svg.hsla);
    let brightness = dot(color.rgb, vec3<f32>(0.30, 0.59, 0.11));
    let k = svg.misc.x * clamp(4.0 * (0.75 - brightness), 0.0, 1.0);
    let enhanced = sample * (k + 1.0) / (sample * k + 1.0);
    let correction = (svg.gamma.x * brightness + svg.gamma.y) * enhanced + (svg.gamma.z * brightness + svg.gamma.w);
    var alpha = (enhanced + enhanced * (1.0 - enhanced) * correction) * color.a * svg.misc.y;
    var clip_id = u32(svg.misc.z);
    loop {
        if (clip_id == 0u) { break; }
        let clip = clips[clip_id - 1u];
        let distance = quad_distance(point, clip.bounds, clip.radii);
        let coverage = select(1.0 - smoothstep(-legacy_aa, legacy_aa, distance), clamp(0.5 - distance, 0.0, 1.0), clip.state.y > 0.5);
        alpha *= select(coverage, select(0.0, 1.0, distance <= 0.0), clip.state.y > 1.5);
        clip_id = u32(clip.state.x);
    }
    return vec4<f32>(color.rgb, alpha);
}
