
struct Uniforms {
    viewport: vec2<f32>,
    pulse_clock_ms: u32,
    pad: u32,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
struct ClipNode { bounds: vec4<f32>, radii: vec4<f32>, state: vec4<f32> }
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
    @location(9) pulse: vec4<u32>,
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
    @location(9) @interpolate(flat) pulse: vec4<u32>,
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
    out.pulse = in.pulse;
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

fn corner_velocity(a: f32, b: f32) -> f32 {
    if (a == 0.0) { return b; }
    if (b == 0.0) { return a; }
    return min(a, b);
}

fn dash_coverage(t: f32, period: f32, dash_length: f32, velocity: f32) -> f32 {
    let centered = (t + period * 0.5 - dash_length * 0.5) % period - period * 0.5;
    return clamp(0.5 - (abs(centered) - dash_length * 0.5) / velocity, 0.0, 1.0);
}

fn dashed_alpha(point: vec2<f32>, size: vec2<f32>, relative: vec2<f32>, corner_point: vec2<f32>, corner: f32, radii: vec4<f32>, widths: vec4<f32>) -> f32 {
    let unrounded = all(radii == vec4<f32>(0.0));
    var t = 0.0;
    var max_t = 0.0;
    var velocity = 0.0;
    if (unrounded) {
        let horizontal = corner_point.x < corner_point.y;
        let width = select(max(widths.y, widths.w), max(widths.z, widths.x), horizontal);
        if (width <= 0.0) { return 1.0; }
        velocity = 1.0 / (3.0 * width);
        t = select(point.y, point.x, horizontal) * velocity;
        max_t = select(size.y, size.x, horizontal) * velocity;
    } else {
        let dt = select(0.0, 1.0 / (3.0 * widths.x), widths.x > 0.0);
        let dr = select(0.0, 1.0 / (3.0 * widths.y), widths.y > 0.0);
        let db = select(0.0, 1.0 / (3.0 * widths.z), widths.z > 0.0);
        let dl = select(0.0, 1.0 / (3.0 * widths.w), widths.w > 0.0);
        let st = (size.x - radii.x - radii.y) * dt;
        let sr = (size.y - radii.y - radii.z) * dr;
        let sb = (size.x - radii.z - radii.w) * db;
        let sl = (size.y - radii.w - radii.x) * dl;
        let dtr = corner_velocity(dt, dr);
        let dbr = corner_velocity(db, dr);
        let dbl = corner_velocity(db, dl);
        let dtl = corner_velocity(dt, dl);
        let utr = st;
        let ur = utr + radii.y * 1.5707963267948966 * dtr;
        let ubr = ur + sr;
        let ub = ubr + radii.z * 1.5707963267948966 * dbr;
        let ubl = ub + sb;
        let ul = ubl + radii.w * 1.5707963267948966 * dbl;
        let utl = ul + sl;
        max_t = utl + radii.x * 1.5707963267948966 * dtl;
        if (corner_point.x >= 0.0 && corner_point.y >= 0.0) {
            let ct = atan2(corner_point.y, corner_point.x) * corner;
            if (relative.x >= 0.0) {
                if (relative.y < 0.0) { velocity = dtr; t = ur - ct * velocity; }
                else { velocity = dbr; t = ubr + ct * velocity; }
            } else {
                if (relative.y >= 0.0) { velocity = dbl; t = ul - ct * velocity; }
                else { velocity = dtl; t = utl + ct * velocity; }
            }
        } else if (corner_point.x < corner_point.y) {
            if (relative.y < 0.0) { velocity = dt; t = (point.x - radii.x) * velocity; }
            else { velocity = db; t = ubl - (point.x - radii.w) * velocity; }
        } else {
            if (relative.x < 0.0) { velocity = dl; t = utl - (point.y - radii.x) * velocity; }
            else { velocity = dr; t = ur + (point.y - radii.y) * velocity; }
        }
    }
    if (velocity == 0.0) { return 1.0; }
    let dash_length = 2.0 / 3.0;
    if (unrounded) { max_t -= dash_length; }
    if (max_t >= 1.0) { return dash_coverage(t, max_t / floor(max_t), dash_length, velocity); }
    if (unrounded && max_t > dash_length) { return dash_coverage(t, max_t, dash_length, velocity); }
    return 1.0;
}

fn quad_color(point: vec2<f32>, inst: Instance) -> vec4<f32> {
    let bounds = inst.rect;
    let half_size = bounds.zw * 0.5;
    let local = point - bounds.xy;
    let relative = local - half_size;
    let radius = corner_radius(relative, inst.shadow);
    let widths = inst.shadow_color;
    var edge = vec2<f32>(select(widths.y, widths.w, relative.x < 0.0), select(widths.z, widths.x, relative.y < 0.0));
    edge = select(edge, vec2<f32>(-0.5), edge == vec2<f32>(0.0));
    let corner_offset = abs(relative) - half_size;
    let corner_point = corner_offset + radius;
    let straight = corner_offset + edge;
    let outer = quad_distance(point, bounds, inst.shadow);
    var inner = 0.0;
    if (corner_point.x <= 0.0 || corner_point.y <= 0.0) {
        inner = -max(straight.x, straight.y);
    } else if (straight.x > 0.0 || straight.y > 0.0) {
        inner = -1.0;
    } else if (edge.x == edge.y) {
        inner = -(outer + edge.x);
    } else {
        let ellipse = max(vec2<f32>(0.0), vec2<f32>(radius) - edge);
        inner = (length(corner_point / ellipse) - 1.0) * (ellipse.x + ellipse.y) * -0.5;
    }
    let border_sdf = max(inner, outer);
    let fill = vec4<f32>(inst.fill.rgb * inst.fill.a, inst.fill.a);
    var color = fill;
    if (border_sdf < 0.5) {
        var alpha = inst.border_color.a;
        if (inst.pad0.y > 0.5) { alpha *= dashed_alpha(local, bounds.zw, relative, corner_point, radius, inst.shadow, widths); }
        let over = vec4<f32>(inst.border_color.rgb * alpha + fill.rgb * (1.0 - alpha), alpha + fill.a * (1.0 - alpha));
        color = mix(fill, over, clamp(0.5 - inner, 0.0, 1.0));
    }
    return color * clamp(0.5 - outer, 0.0, 1.0);
}

fn gpui_hsla(color: vec4<f32>) -> vec4<f32> {
    let h = color.x * 6.0;
    let c = (1.0 - abs(2.0 * color.z - 1.0)) * color.y;
    let x = c * (1.0 - abs((h % 2.0) - 1.0));
    let m = color.z - c * 0.5;
    var rgb = vec3<f32>(c, 0.0, x);
    if (h >= 0.0 && h < 1.0) { rgb = vec3<f32>(c, x, 0.0); }
    else if (h >= 1.0 && h < 2.0) { rgb = vec3<f32>(x, c, 0.0); }
    else if (h >= 2.0 && h < 3.0) { rgb = vec3<f32>(0.0, c, x); }
    else if (h >= 3.0 && h < 4.0) { rgb = vec3<f32>(0.0, x, c); }
    else if (h >= 4.0 && h < 5.0) { rgb = vec3<f32>(x, 0.0, c); }
    return vec4<f32>(rgb + m, color.a);
}

fn gpui_linear(rgb: vec3<f32>) -> vec3<f32> {
    return select(pow((rgb + 0.055) / 1.055, vec3<f32>(2.4)), rgb / 12.92, rgb <= vec3<f32>(0.04045));
}

fn gpui_encoded(rgb: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(rgb, vec3<f32>(1.0 / 2.4)) - 0.055, rgb * 12.92, rgb <= vec3<f32>(0.0031308));
}

fn gpui_to_oklab(color: vec4<f32>) -> vec4<f32> {
    let rgb = gpui_linear(color.rgb);
    let l = pow(dot(rgb, vec3<f32>(0.4122214708, 0.5363325363, 0.0514459929)), 1.0 / 3.0);
    let m = pow(dot(rgb, vec3<f32>(0.2119034982, 0.6806995451, 0.1073969566)), 1.0 / 3.0);
    let s = pow(dot(rgb, vec3<f32>(0.0883024619, 0.2817188376, 0.6299787005)), 1.0 / 3.0);
    return vec4<f32>(0.2104542553*l + 0.7936177850*m - 0.0040720468*s, 1.9779984951*l - 2.4285922050*m + 0.4505937099*s, 0.0259040371*l + 0.7827717662*m - 0.8086757660*s, color.a);
}

fn gpui_from_oklab(color: vec4<f32>) -> vec4<f32> {
    let l0 = color.x + 0.3963377774*color.y + 0.2158037573*color.z;
    let m0 = color.x - 0.1055613458*color.y - 0.0638541728*color.z;
    let s0 = color.x - 0.0894841775*color.y - 1.2914855480*color.z;
    let l = l0*l0*l0; let m = m0*m0*m0; let s = s0*s0*s0;
    let rgb = clamp(vec3<f32>(4.0767416621*l - 3.3077115913*m + 0.2309699292*s, -1.2684380046*l + 2.6097574011*m - 0.3413193965*s, -0.0041960863*l - 0.7034186147*m + 1.7076147010*s), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(gpui_encoded(rgb), color.a);
}

fn gpui_dither(point: vec2<f32>) -> f32 {
    let x = i32(floor(point.x)) & 3;
    let y = i32(floor(point.y)) & 3;
    let qx = (x >> 1) & 1; let qy = (y >> 1) & 1;
    let lx = x & 1; let ly = y & 1;
    let threshold = 4 * (2 * (lx ^ ly) + ly) + 2 * (qx ^ qy) + qy;
    return (f32(threshold) + 0.5) / 16.0 - 0.5;
}

fn gpui_gradient_color(point: vec2<f32>, inst: Instance) -> vec4<f32> {
    var first = gpui_hsla(inst.fill);
    var color = first;
    if (inst.pad0.y > 0.5) {
        var second = gpui_hsla(inst.border_color);
        if (inst.radius_bw.y > 0.5) { first = gpui_to_oklab(first); second = gpui_to_oklab(second); }
        let radians = ((inst.radius_bw.x % 360.0) - 90.0) * 0.017453292519943295;
        var direction = vec2<f32>(cos(radians), sin(radians));
        let size = inst.rect.zw;
        if (size.x > size.y) { direction.y *= size.y / size.x; }
        else { direction.x *= size.x / size.y; }
        let half_size = size * 0.5;
        var t = dot(point - inst.rect.xy - half_size, direction) / length(direction);
        if (abs(direction.x) > abs(direction.y)) { t = (t + half_size.x) / size.x; }
        else { t = (t + half_size.y) / size.y; }
        t = clamp((t - inst.shadow_color.x) / (inst.shadow_color.y - inst.shadow_color.x), 0.0, 1.0);
        color = mix(first, second, t);
        if (inst.radius_bw.y > 0.5) { color = gpui_from_oklab(color); }
        if (any(first != second)) { color = vec4<f32>(color.rgb + gpui_dither(point) / 255.0, color.a); }
    }
    return color;
}

fn gpui_background(point: vec2<f32>, inst: Instance) -> vec4<f32> {
    let color = gpui_gradient_color(point, inst);
    let alpha = color.a * clamp(0.5 - quad_distance(point, inst.rect, inst.shadow), 0.0, 1.0);
    return vec4<f32>(color.rgb * alpha, alpha);
}

fn rect_fragment(in: VsOut) -> vec4<f32> {
    let inst = Instance(in.rect, in.rb_pack.xy, in.rb_pack.zw, in.fill,
                        in.border_color, in.shadow, in.shadow_color,
                        in.clip, in.misc);
    var opacity = inst.misc.y;
    if (in.pulse.y != 0u) {
        let progress = f32((u.pulse_clock_ms - in.pulse.x) % in.pulse.y) / f32(in.pulse.y);
        let swing = cos(progress * 6.28318530718) * 0.5 + 0.5;
        let minimum = bitcast<f32>(in.pulse.z);
        opacity *= minimum + (1.0 - minimum) * swing;
    }

    var clip_cover = 1.0;
    var clip_id = u32(inst.misc.z);
    let clip_aa = max(fwidth(in.px.x), fwidth(in.px.y)) * 1.5 + 1e-4;
    loop {
        if (clip_id == 0u) { break; }
        let clip = clips[clip_id - 1u];
        let distance = quad_distance(in.px, clip.bounds, clip.radii);
        let legacy_coverage = 1.0 - smoothstep(-clip_aa, clip_aa, distance);
        let exact_coverage = clamp(0.5 - distance, 0.0, 1.0);
        let coverage = select(legacy_coverage, exact_coverage, clip.state.y > 0.5);
        clip_cover *= select(coverage, select(0.0, 1.0, distance <= 0.0), clip.state.y > 1.5);
        clip_id = u32(clip.state.x);
    }
    if (inst.pad0.x > 4.5) {
        let coverage = shadow_alpha(in.px, inst.fill, inst.shadow, inst.border_color, inst.radius_bw.x, inst.radius_bw.y > 0.5, inst.pad0.y);
        let color = gpui_hsla(inst.shadow_color);
        let alpha = coverage * color.a * clip_cover * opacity;
        return vec4<f32>(color.rgb * alpha, alpha);
    }
    if (inst.pad0.x > 3.5) {
        var encoded = inst;
        encoded.border_color = gpui_hsla(inst.border_color);
        return quad_color(in.px, encoded) * clip_cover * opacity;
    }
    if (inst.pad0.x > 2.5) {
        return gpui_background(in.px, inst) * clip_cover * opacity;
    }
    if (inst.pad0.x > 1.5) {
        return quad_color(in.px, inst) * clip_cover * opacity;
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
        fill_rgb = inst.fill.rgb * inner_cover + inst.border_color.rgb * band;
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return rect_fragment(in);
}

@fragment
fn fs_gpui(in: VsOut) -> @location(0) vec4<f32> {
    let premultiplied = rect_fragment(in);
    if (in.rb_pack.z > 4.5) {
        return vec4<f32>(gpui_hsla(in.shadow_color).rgb, premultiplied.a);
    }
    if (in.rb_pack.z > 3.5) {
        return vec4<f32>(gpui_hsla(in.border_color).rgb, premultiplied.a);
    }
    if (in.rb_pack.z > 2.5) {
        let inst = Instance(in.rect, in.rb_pack.xy, in.rb_pack.zw, in.fill,
                            in.border_color, in.shadow, in.shadow_color,
                            in.clip, in.misc);
        return vec4<f32>(gpui_gradient_color(in.px, inst).rgb, premultiplied.a);
    }
    if (premultiplied.a <= 0.0) { return vec4<f32>(0.0); }
    return vec4<f32>(premultiplied.rgb / premultiplied.a, premultiplied.a);
}
