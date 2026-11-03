struct Uniforms {
    viewport: vec2<f32>,
    progress: f32,
    band: f32,
    tint: vec4<f32>,
    mode: u32,
    padding0: u32,
    padding1: u32,
    padding2: u32,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var current: texture_2d<f32>;
@group(0) @binding(2) var previous: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
struct Output { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> Output {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: Output;
    output.pos = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    output.uv = uv;
    return output;
}
fn encoded(rgb: vec3<f32>) -> vec3<f32> {
    return select(rgb * 12.92, 1.055 * pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055, rgb > vec3<f32>(0.0031308));
}
fn linear(rgb: vec3<f32>) -> vec3<f32> {
    return select(rgb / 12.92, pow(max((rgb + 0.055) / 1.055, vec3<f32>(0.0)), vec3<f32>(2.4)), rgb > vec3<f32>(0.04045));
}
@fragment fn fs_main(in: Output) -> @location(0) vec4<f32> {
    let value = textureSample(current, smp, in.uv);
    if u.mode == 1u {
        let offset = (in.uv.x * u.viewport.x - u.progress) / max(u.band - u.progress, 0.0001);
        let amount = clamp(min(offset / 0.4, (1.0 - offset) / 0.4), 0.0, 1.0) * u.tint.a;
        let straight = value.rgb / max(value.a, 0.00001);
        return vec4<f32>(linear(mix(encoded(straight), encoded(u.tint.rgb), amount)) * value.a, value.a);
    }
    let old = textureSample(previous, smp, in.uv);
    let diagonal = (in.uv.x + in.uv.y) * 0.5;
    let edge = mix(-u.band, 1.0 + u.band, u.progress);
    let reveal = 1.0 - smoothstep(edge - u.band * 0.5, edge + u.band * 0.5, diagonal);
    let mixed = mix(old, value, reveal);
    let shimmer = (1.0 - abs(2.0 * reveal - 1.0)) * u.tint.a;
    return vec4<f32>(mix(mixed.rgb, u.tint.rgb * mixed.a, shimmer), mixed.a);
}
