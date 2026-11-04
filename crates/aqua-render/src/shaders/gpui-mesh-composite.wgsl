struct Composite {
    bounds: vec4<f32>,
    clip: vec4<f32>,
    corners: vec4<f32>,
    params: vec4<f32>,
    viewport: vec4<f32>,
}

@group(0) @binding(0) var<uniform> composite: Composite;
@group(0) @binding(1) var mesh_texture: texture_2d<f32>;
@group(0) @binding(2) var mesh_sampler: sampler;
struct Clip { bounds: vec4<f32>, corners: vec4<f32>, state: vec4<f32> }
@group(0) @binding(3) var<storage, read> clips: array<Clip>;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) clip_distances: vec4<f32>,
}

@vertex
fn composite_vertex(@builtin(vertex_index) vertex_id: u32) -> Output {
    let unit = vec2<f32>(f32(vertex_id & 1u), f32((vertex_id >> 1u) & 1u));
    let position = composite.bounds.xy + unit * composite.bounds.zw;
    var output: Output;
    output.position = vec4<f32>(2.0 * position.x / composite.viewport.x - 1.0, 1.0 - 2.0 * position.y / composite.viewport.y, 0.0, 1.0);
    output.uv = unit;
    output.clip_distances = vec4<f32>(position - composite.clip.xy, composite.clip.xy + composite.clip.zw - position);
    return output;
}

@fragment
fn composite_fragment(input: Output) -> @location(0) vec4<f32> {
    if (any(input.clip_distances < vec4<f32>(0.0))) { return vec4<f32>(0.0); }
    let relative = input.position.xy - (composite.bounds.xy + composite.bounds.zw * 0.5);
    var radius = select(composite.corners.z, composite.corners.y, relative.y < 0.0);
    if (relative.x < 0.0) { radius = select(composite.corners.w, composite.corners.x, relative.y < 0.0); }
    let q = abs(relative) - composite.bounds.zw * 0.5 + radius;
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    var coverage = clamp(0.5 - distance, 0.0, 1.0);
    for (var index = 0u; index < u32(composite.params.y); index += 1u) {
        let clip = clips[index];
        let p = input.position.xy - clip.bounds.xy - clip.bounds.zw * 0.5;
        var r = select(clip.corners.z, clip.corners.y, p.y < 0.0);
        if (p.x < 0.0) { r = select(clip.corners.w, clip.corners.x, p.y < 0.0); }
        let q_clip = abs(p) - clip.bounds.zw * 0.5 + r;
        let d = select(length(max(q_clip, vec2<f32>(0.0))) + min(max(q_clip.x, q_clip.y), 0.0) - r, max(q_clip.x, q_clip.y), r == 0.0);
        let aa = 1.5001;
        let shape = select(1.0 - smoothstep(-aa, aa, d), clamp(0.5 - d, 0.0, 1.0), clip.state.x > 0.5);
        coverage *= select(shape, select(0.0, 1.0, d <= 0.0), clip.state.x > 1.5);
    }
    return textureSampleLevel(mesh_texture, mesh_sampler, input.uv, 0.0) * (composite.params.x * coverage);
}
