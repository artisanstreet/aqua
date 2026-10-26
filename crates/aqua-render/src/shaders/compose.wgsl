// Compose: textured quad with opacity + rounded-clip, premultiplied-over.
// Used for base->surface blits, blurred-region quads, and external layers
// (media / viewport frames).
//
// Alpha convention: internal targets (base, blur/group scratch) are
// PREMULTIPLIED working space. Media and viewport owners normalize their
// external layers to the same convention. Explicit straight-alpha imports
// use premultiplied = 0 so the shader multiplies rgb*a before compositing.
// Straight input through premultiplied math renders too bright at half
// alpha — the flag exists so the difference is explicit, not hidden.

struct ComposeUniforms {
    viewport: vec2<f32>, // target size in physical px
    opacity: f32,
    clip_radius: f32,
    output_gamma: f32,   // 0 = linear, 1 = encoded premultiplied, 2 = encoded premultiplied via sRGB target
    premultiplied: f32,  // 1 = source already premultiplied, 0 = straight
    pad: vec2<f32>,      // explicit pad: struct is exactly 8 floats / 32 B
}
@group(0) @binding(0) var<uniform> u: ComposeUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_smp: sampler;

struct VsIn {
    @location(0) corner: vec2<f32>,   // per-vertex 0..1 (8-byte stride buffer)
    @location(1) dst_rect: vec4<f32>, // per-instance: x, y, w, h in physical px
    @location(2) src_rect: vec4<f32>, // per-instance: uv-space source rect
    @location(3) clip: vec4<f32>,     // per-instance: x, y, w, h; w < 0 disables
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
    // Rounded-clip edge coverage multiplies alpha (real AA on the mask
    // edge), never a hard discard threshold.
    var clip_cover = 1.0;
    if (in.clip.z >= 0.0) {
        let cc = in.clip.xy + in.clip.zw * 0.5;
        let d = sd_round_box(in.px - cc, in.clip.zw * 0.5, u.clip_radius);
        let aa = fwidth(d) * 1.5 + 1e-4;
        clip_cover = 1.0 - smoothstep(-aa, aa, d);
        if (clip_cover <= 0.001) { discard; }
    }
    let c = textureSample(src_tex, src_smp, in.uv);
    var rgb = c.rgb;
    var a = c.a;
    if (u.premultiplied < 0.5) {
        rgb = c.rgb * c.a;
    }
    let coverage = u.opacity * clip_cover;
    if (u.output_gamma > 0.5) {
        // Compositors expect premultiplied ENCODED channels. Encoding
        // premultiplied linear channels directly creates bright halos.
        var straight = vec3<f32>(0.0);
        if (a > 0.0) { straight = max(rgb / a, vec3<f32>(0.0)); }
        let encoded = select(straight * 12.92,
            1.055 * pow(straight, vec3<f32>(1.0 / 2.4)) - 0.055,
            straight > vec3<f32>(0.0031308));
        let premultiplied = encoded * a * coverage;
        if (u.output_gamma > 1.5) {
            // An sRGB attachment performs a final hardware encode. Undo
            // it here so its stored bytes are still encoded-premultiplied.
            let linear = select(premultiplied / 12.92,
                pow(max((premultiplied + 0.055) / 1.055, vec3<f32>(0.0)), vec3<f32>(2.4)),
                premultiplied > vec3<f32>(0.04045));
            return vec4<f32>(linear, a * coverage);
        }
        return vec4<f32>(premultiplied, a * coverage);
    }
    return vec4<f32>(rgb * coverage, a * coverage);
}
