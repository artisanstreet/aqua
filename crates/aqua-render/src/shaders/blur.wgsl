// Separable Gaussian blur: 9 taps per pass, two passes (H then V).
// Samples the source (scene snapshot) texture; writes blurred output.

struct BlurUniforms {
    texel: vec2<f32>,     // 1 / source texture size
    direction: vec2<f32>, // (1,0) horizontal, (0,1) vertical
    sigma: f32,
    step: f32,            // tap spacing in texels (kernel covers ±4*step)
    pad0: vec2<f32>,
}
@group(0) @binding(0) var<uniform> u: BlurUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_smp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    // Big-triangle fullscreen cover: all three vertices map NDC top-left
    // (-1,+1) to uv (0,0), consistent with the rect/compose convention
    // (uv.y = 0 at the top row). Off-screen vertices (uv 2,0 / 0,2)
    // interpolate correctly across the visible area.
    var p = vec2<f32>(-1.0, 1.0);
    var uv = vec2<f32>(0.0, 0.0);
    if (vi == 1u) {
        p = vec2<f32>(3.0, 1.0);
        uv = vec2<f32>(2.0, 0.0);
    } else if (vi == 2u) {
        p = vec2<f32>(-1.0, -3.0);
        uv = vec2<f32>(0.0, 2.0);
    }
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if (u.sigma < 0.25) {
        return textureSample(src_tex, src_smp, in.uv);
    }
    // 9-tap Gaussian with adaptive step: covers ±4*step texels ≈ ±2σ.
    let s2 = u.sigma * u.sigma;
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var i: i32 = -4; i <= 4; i++) {
        let fi = f32(i) * u.step;
        let w = exp(-0.5 * fi * fi / s2);
        let off = u.direction * u.texel * fi;
        acc += textureSample(src_tex, src_smp, in.uv + off) * w;
        wsum += w;
    }
    return acc / wsum;
}
