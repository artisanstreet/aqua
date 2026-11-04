@group(1) @binding(0) var<uniform> path_instance: Instance;
@group(2) @binding(0) var path_texture: texture_2d<f32>;

struct PathVertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn path_vertex(@location(0) position: vec2<f32>) -> PathVertexOutput {
    var output: PathVertexOutput;
    output.position = vec4<f32>(position.x / u.viewport.x * 2.0 - 1.0, 1.0 - position.y / u.viewport.y * 2.0, 0.0, 1.0);
    return output;
}

@fragment
fn path_fragment(input: PathVertexOutput) -> @location(0) vec4<f32> {
    let point = input.position.xy;
    let legacy_aa = max(fwidth(point.x), fwidth(point.y)) * 1.5 + 1e-4;
    let bounds = path_instance.clip;
    if (point.x < bounds.x || point.y < bounds.y || point.x >= bounds.x + bounds.z || point.y >= bounds.y + bounds.w) { discard; }
    var coverage = 1.0;
    var clip_id = u32(path_instance.misc.z);
    loop {
        if (clip_id == 0u) { break; }
        let clip = clips[clip_id - 1u];
        let distance = quad_distance(point, clip.bounds, clip.radii);
        let clip_coverage = select(1.0 - smoothstep(-legacy_aa, legacy_aa, distance), clamp(0.5 - distance, 0.0, 1.0), clip.state.y > 0.5);
        coverage *= select(clip_coverage, select(0.0, 1.0, distance <= 0.0), clip.state.y > 1.5);
        clip_id = u32(clip.state.x);
    }
    let color = gpui_gradient_color(point, path_instance);
    let alpha = color.a * coverage;
    return vec4<f32>(color.rgb * alpha, alpha);
}

@vertex
fn path_sprite_vertex(@builtin(vertex_index) index: u32) -> PathVertexOutput {
    let corners = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
    let xy = path_instance.clip.xy + corners[index] * path_instance.clip.zw;
    var output: PathVertexOutput;
    output.position = vec4<f32>(xy.x / u.viewport.x * 2.0 - 1.0, 1.0 - xy.y / u.viewport.y * 2.0, 0.0, 1.0);
    return output;
}

@fragment
fn path_sprite_fragment(input: PathVertexOutput) -> @location(0) vec4<f32> {
    return textureLoad(path_texture, vec2<i32>(input.position.xy), 0);
}
