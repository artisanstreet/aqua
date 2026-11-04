struct BlurUniforms {
    texel: vec2<f32>,
    direction: vec2<f32>,
    sigma: f32,
    step: f32,
    mode: f32,
    taps: f32,
    source_origin: vec2<f32>,
    padding: vec2<f32>,
}
@group(0) @binding(0) var<uniform> u: BlurUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_smp: sampler;

struct Output {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Output {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: Output;
    output.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    output.uv = uv;
    return output;
}

@fragment
fn fs_main(input: Output) -> @location(0) vec4<f32> {
    if (u.mode < 0.5) {
        let size = vec2<f32>(textureDimensions(src_tex));
        let uv = (floor(input.pos.xy) * 2.0 + 1.0 - u.source_origin) / size;
        return textureSampleLevel(src_tex, src_smp, uv, 0.0);
    }
    var color = vec4<f32>(0.0);
    var sum = 0.0;
    let taps = i32(u.taps);
    for (var i = -taps; i <= taps; i++) {
        let offset = f32(i) * u.step;
        let weight = exp(-0.5 * offset * offset / (u.sigma * u.sigma));
        color += textureSampleLevel(src_tex, src_smp, input.uv + u.direction * u.texel * offset, 0.0) * weight;
        sum += weight;
    }
    return color / max(sum, 0.00001);
}
