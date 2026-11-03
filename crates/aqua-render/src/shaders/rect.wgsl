
struct Uniforms {
    viewport: vec2<f32>,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
struct ClipNode { bounds: vec4<f32>, state: vec4<f32> }
@group(0) @binding(1) var<storage, read> clips: array<ClipNode>;

struct Instance {
    rect: vec4<f32>,
    radius_bw: vec2<f32>,
    pad0: vec2<f32>,
    fill: vec4<f32>,
    border_color: vec4<f32>,
    shadow: vec4<f32>,
    shadow_color: vec4<f32>,
    clip: vec4<f32>,
    misc: vec4<f32>,
}

struct VsIn {
    @location(0) corner: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) rb_pack: vec4<f32>,
    @location(3) fill: vec4<f32>,
    @location(4) border_color: vec4<f32>,
    @location(5) shadow: vec4<f32>,
    @location(6) shadow_color: vec4<f32>,
    @location(7) clip: vec4<f32>,
    @location(8) misc: vec4<f32>,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) px: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) rb_pack: vec4<f32>,
    @location(3) fill: vec4<f32>,
    @location(4) border_color: vec4<f32>,
    @location(5) shadow: vec4<f32>,
    @location(6) shadow_color: vec4<f32>,
    @location(7) clip: vec4<f32>,
    @location(8) misc: vec4<f32>,
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let p = vec2<f32>(in.rect.x + in.corner.x * in.rect.z,
                      in.rect.y + in.corner.y * in.rect.w);
    out.px = p;
    let ndc = vec2<f32>(p.x / u.viewport.x * 2.0 - 1.0,
                        1.0 - p.y / u.viewport.y * 2.0);
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.rect = in.rect;
    out.rb_pack = in.rb_pack;
    out.fill = in.fill;
    out.border_color = in.border_color;
    out.shadow = in.shadow;
    out.shadow_color = in.shadow_color;
    out.clip = in.clip;
    out.misc = in.misc;
    return out;
}

fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + r;
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn corner_radius(point: vec2<f32>, radii: vec4<f32>) -> f32 {
    if (point.x < 0.0) {
        return select(radii.w, radii.x, point.y < 0.0);
    }
    return select(radii.z, radii.y, point.y < 0.0);
}

fn quad_distance(point: vec2<f32>, bounds: vec4<f32>, radii: vec4<f32>) -> f32 {
    let half_size = bounds.zw * 0.5;
    let relative = point - bounds.xy - half_size;
    let radius = corner_radius(relative, radii);
    let q = abs(relative) - half_size + radius;
    if (radius == 0.0) {
        return max(q.x, q.y);
    }
    return length(max(q, vec2<f32>(0.0))) + min(0.0, max(q.x, q.y)) - radius;
}

fn error_function(value: vec2<f32>) -> vec2<f32> {
    let sign_value = sign(value);
    let a = abs(value);
    var x = vec2<f32>(1.0) + (vec2<f32>(0.278393) + (vec2<f32>(0.230389) + 0.078108 * a * a) * a) * a;
    x *= x;
    return sign_value - sign_value / (x * x);
}

fn blur_along_x(x: f32, y: f32, sigma: f32, corner: f32, half_size: vec2<f32>) -> f32 {
    let delta = min(half_size.y - corner - abs(y), 0.0);
    let curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = vec2<f32>(0.5) + 0.5 * error_function((vec2<f32>(x) + vec2<f32>(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

fn shadow_alpha(point: vec2<f32>, element: vec4<f32>, bounds: vec4<f32>, radii: vec4<f32>, sigma: f32, inset: bool, spread: f32) -> f32 {
    let hole_radii = select(radii, max(radii - vec4<f32>(spread), vec4<f32>(0.0)), inset);
    let half_size = bounds.zw * 0.5;
    let relative = point - bounds.xy - half_size;
    let corner = corner_radius(relative, hole_radii);
    var alpha = 0.0;
    if (sigma == 0.0) {
        alpha = clamp(0.5 - quad_distance(point, bounds, hole_radii), 0.0, 1.0);
    } else {
        let low = relative.y - half_size.y;
        let high = relative.y + half_size.y;
        let start = clamp(-3.0 * sigma, low, high);
        let end = clamp(3.0 * sigma, low, high);
        let step = (end - start) / 4.0;
        var y = start + step * 0.5;
        for (var i = 0; i < 4; i++) {
            let gaussian = exp(-(y * y) / (2.0 * sigma * sigma)) / (sqrt(2.0 * 3.141592653589793) * sigma);
            alpha += blur_along_x(relative.x, relative.y - y, sigma, corner, half_size) * gaussian * step;
            y += step;
        }
    }
    let element_distance = quad_distance(point, element, radii);
    if (inset) {
        return (1.0 - alpha) * clamp(0.5 - element_distance, 0.0, 1.0);
    }
    return alpha * clamp(0.5 + element_distance, 0.0, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let inst = Instance(in.rect, in.rb_pack.xy, in.rb_pack.zw, in.fill,
                        in.border_color, in.shadow, in.shadow_color,
                        in.clip, in.misc);
    let opacity = inst.misc.y;

    var clip_cover = 1.0;
    var clip_id = u32(inst.misc.z);
    let clip_aa = max(fwidth(in.px.x), fwidth(in.px.y)) * 1.5 + 1e-4;
    loop {
        if (clip_id == 0u) { break; }
        let clip = clips[clip_id - 1u];
        let center = clip.bounds.xy + clip.bounds.zw * 0.5;
        let distance = sd_round_box(in.px - center, clip.bounds.zw * 0.5, clip.state.x);
        clip_cover *= 1.0 - smoothstep(-clip_aa, clip_aa, distance);
        clip_id = u32(clip.state.y);
    }
    if (inst.pad0.x > 0.5) {
        let coverage = shadow_alpha(in.px, inst.fill, inst.shadow, inst.border_color, inst.radius_bw.x, inst.radius_bw.y > 0.5, inst.pad0.y);
        let alpha = coverage * inst.shadow_color.a * clip_cover * opacity;
        return vec4<f32>(inst.shadow_color.rgb * alpha, alpha);
    }
    let rc = inst.rect.xy + inst.rect.zw * 0.5;
    let half = inst.rect.zw * 0.5;
    let rad = min(inst.radius_bw.x, min(half.x, half.y));

    let d = sd_round_box(in.px - rc, half, rad);
    let aa = fwidth(d) * 1.5 + 1e-4;
    let fill_cover = 1.0 - smoothstep(-aa, aa, d);
    var fill_a = fill_cover;
    var fill_rgb = inst.fill.rgb;
    let bw = inst.radius_bw.y;
    if (bw > 0.001) {
        let d_in = sd_round_box(in.px - rc, half - vec2<f32>(bw, bw), max(rad - bw, 0.0));
        let inner_cover = min(fill_cover, 1.0 - smoothstep(-aa, aa, d_in));
        let band = max(fill_cover - inner_cover, 0.0);
        fill_rgb = inst.fill.rgb * inst.fill.a * inner_cover + inst.border_color.rgb * inst.border_color.a * band;
        fill_a = inst.fill.a * inner_cover + inst.border_color.a * band;
    } else {
        fill_rgb = inst.fill.rgb * fill_cover;
        fill_a = fill_cover * inst.fill.a;
    }

    var out_rgb = fill_rgb;
    var out_a = fill_a;
    if (inst.shadow.w > 0.5 && inst.shadow.z > 0.001) {
        let sc = rc + inst.shadow.xy;
        let ds = sd_round_box(in.px - sc, half, rad);
        let sigma = max(inst.shadow.z * 0.5, 0.5);
        let shadow_cover = (1.0 - smoothstep(-sigma * 2.0, sigma * 2.0, ds)) * (1.0 - fill_a);
        let sa = shadow_cover * inst.shadow_color.a;
        out_rgb = inst.shadow_color.rgb * sa + fill_rgb * (1.0 - sa);
        out_a = sa + fill_a * (1.0 - sa);
    }

    out_rgb *= clip_cover;
    out_a *= clip_cover;

    return vec4<f32>(out_rgb * opacity, out_a * opacity);
}
