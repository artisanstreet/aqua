
struct ComposeUniforms {
    viewport: vec2<f32>,
    opacity: f32,
    clip_radius: f32,
    output_gamma: f32,
    premultiplied: f32,
    pad: vec2<f32>,
}
@group(0) @binding(0) var<uniform> u: ComposeUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_smp: sampler;
struct Clip { bounds: vec4<f32>, radius: vec4<f32> }
@group(0) @binding(3) var<storage, read> clips: array<Clip>;

struct VsIn {
    @location(0) corner: vec2<f32>,
    @location(1) dst_rect: vec4<f32>,
    @location(2) src_rect: vec4<f32>,
    @location(3) clip: vec4<f32>,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) px: vec2<f32>,
    @location(2) clip: vec4<f32>,
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let p = vec2<f32>(in.dst_rect.x + in.corner.x * in.dst_rect.z,
                      in.dst_rect.y + in.corner.y * in.dst_rect.w);
    out.px = p;
    out.uv = in.src_rect.xy + in.corner * in.src_rect.zw;
    out.clip = in.clip;
    let ndc = vec2<f32>(p.x / u.viewport.x * 2.0 - 1.0,
                        1.0 - p.y / u.viewport.y * 2.0);
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    return out;
}

fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + r;
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var clip_cover = 1.0;
    if (in.clip.z >= 0.0) {
        let cc = in.clip.xy + in.clip.zw * 0.5;
        let d = sd_round_box(in.px - cc, in.clip.zw * 0.5, u.clip_radius);
        let aa = fwidth(d) * 1.5 + 1e-4;
        clip_cover = 1.0 - smoothstep(-aa, aa, d);

    }
    for (var index = 0u; index < u32(u.pad.x); index += 1u) {
        let clip = clips[index];
        let center = clip.bounds.xy + clip.bounds.zw * 0.5;
        let distance = sd_round_box(in.px - center, clip.bounds.zw * 0.5, clip.radius.x);
        let aa = fwidth(distance) * 1.5 + 1e-4;
        clip_cover *= 1.0 - smoothstep(-aa, aa, distance);
    }
    if (clip_cover <= 0.001) { discard; }
    let c = textureSample(src_tex, src_smp, in.uv);
    var rgb = c.rgb;
    var a = c.a;
    if (u.premultiplied < 0.5) {
        rgb = c.rgb * c.a;
    }
    let coverage = u.opacity * clip_cover;
    if (u.output_gamma > 0.5) {
        var straight = vec3<f32>(0.0);
        if (a > 0.0) { straight = max(rgb / a, vec3<f32>(0.0)); }
        let encoded = select(straight * 12.92,
            1.055 * pow(straight, vec3<f32>(1.0 / 2.4)) - 0.055,
            straight > vec3<f32>(0.0031308));
        let premultiplied = encoded * a * coverage;
        if (u.output_gamma > 1.5) {
            let linear = select(premultiplied / 12.92,
                pow(max((premultiplied + 0.055) / 1.055, vec3<f32>(0.0)), vec3<f32>(2.4)),
                premultiplied > vec3<f32>(0.04045));
            return vec4<f32>(linear, a * coverage);
        }
        return vec4<f32>(premultiplied, a * coverage);
    }
    return vec4<f32>(rgb * coverage, a * coverage);
}
