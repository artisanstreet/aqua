// Rounded-rect primitive: fill + border + drop shadow + clip + opacity.
// One instanced quad per DisplayItem::Rect. The quad covers the union of the
// rect and its shadow footprint (computed on the CPU); the shader composites
// shadow under fill within a single fragment output (premultiplied-over).

struct Uniforms {
    viewport: vec2<f32>, // physical pixels
}
@group(0) @binding(0) var<uniform> u: Uniforms;

struct Instance {
    rect: vec4<f32>,        // x, y, w, h (physical px, top-left origin)
    radius_bw: vec2<f32>,   // corner radius, border width
    pad0: vec2<f32>,
    fill: vec4<f32>,        // premultiplied linear
    border_color: vec4<f32>,// premultiplied linear
    shadow: vec4<f32>,      // offset_x, offset_y, blur, enabled
    shadow_color: vec4<f32>,// premultiplied linear
    clip: vec4<f32>,        // x, y, w, h; w < 0 disables
    misc: vec4<f32>,        // clip_radius, opacity, 0, 0
}

struct VsIn {
    @location(0) corner: vec2<f32>, // 0..1 across the instance quad
    @location(1) rect: vec4<f32>,
    @location(2) rb_pack: vec4<f32>, // radius, border_width, 0, 0
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
    // top-left px origin -> NDC
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let inst = Instance(in.rect, in.rb_pack.xy, in.rb_pack.zw, in.fill,
                        in.border_color, in.shadow, in.shadow_color,
                        in.clip, in.misc);
    let opacity = inst.misc.y;

    // Axis clip (hard discard outside, AA on rounded clip).
    if (inst.clip.z >= 0.0) {
        let cc = inst.clip.xy + inst.clip.zw * 0.5;
        let d = sd_round_box(in.px - cc, inst.clip.zw * 0.5, inst.misc.x);
        let aa = fwidth(d) * 1.5 + 1e-4;
        let cover = 1.0 - smoothstep(-aa, aa, d);
        if (cover <= 0.001) { discard; }
    }

    let rc = inst.rect.xy + inst.rect.zw * 0.5; // rect center
    let half = inst.rect.zw * 0.5;
    let rad = min(inst.radius_bw.x, min(half.x, half.y));

    // Fill + border coverage.
    let d = sd_round_box(in.px - rc, half, rad);
    let aa = fwidth(d) * 1.5 + 1e-4;
    let fill_cover = 1.0 - smoothstep(-aa, aa, d);
    var fill_a = fill_cover;
    var fill_rgb = inst.fill.rgb;
    let bw = inst.radius_bw.y;
    if (bw > 0.001) {
        let d_in = sd_round_box(in.px - rc, half - vec2<f32>(bw, bw), max(rad - bw, 0.0));
        // A border is the outer coverage minus the inset coverage.
        // Subtracting SDF distances instead produces a constant everywhere
        // inside the shape and incorrectly paints its entire fill as border.
        let inner_cover = min(fill_cover, 1.0 - smoothstep(-aa, aa, d_in));
        let band = max(fill_cover - inner_cover, 0.0);
        fill_rgb = inst.fill.rgb * inner_cover + inst.border_color.rgb * band;
        fill_a = inst.fill.a * inner_cover + inst.border_color.a * band;
    } else {
        fill_rgb = inst.fill.rgb * fill_cover;
        fill_a = fill_cover * inst.fill.a;
    }

    // Drop shadow under fill.
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

    // Rounded-clip coverage multiplies alpha.
    if (inst.clip.z >= 0.0 && inst.misc.x > 0.001) {
        let cc = inst.clip.xy + inst.clip.zw * 0.5;
        let dc = sd_round_box(in.px - cc, inst.clip.zw * 0.5, inst.misc.x);
        let aac = fwidth(dc) * 1.5 + 1e-4;
        let ccov = 1.0 - smoothstep(-aac, aac, dc);
        out_rgb *= ccov;
        out_a *= ccov;
    }

    return vec4<f32>(out_rgb * opacity, out_a * opacity);
}
