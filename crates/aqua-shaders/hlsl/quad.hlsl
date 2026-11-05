#include "common.hlsli"

struct QuadIn
{
    float4 bounds : BOUNDS;
    float4 clip : CLIP;
    float4 shape : SHAPE;
    float4 fill : FILL;
    float4 border : BORDER;
    float4 gradient : GRADIENT;
    float4 stop_a : STOPA;
    float4 stop_b : STOPB;
    float4 stop_offsets : STOPOFFSETS;
    float4 xform : XFORM;
    uint vertex : SV_VertexID;
};

struct QuadOut
{
    float4 position : SV_Position;
    nointerpolation float4 bounds : BOUNDS;
    nointerpolation float4 clip : CLIP;
    nointerpolation float4 shape : SHAPE;
    nointerpolation float4 fill : FILL;
    nointerpolation float4 border : BORDER;
    nointerpolation float4 gradient : GRADIENT;
    nointerpolation float4 stop_a : STOPA;
    nointerpolation float4 stop_b : STOPB;
    nointerpolation float4 stop_offsets : STOPOFFSETS;
    nointerpolation float4 xform : XFORM;
};

QuadOut vs_quad(QuadIn i)
{
    QuadOut o;
    float2 corner = float2(i.vertex & 1, i.vertex >> 1);
    float2 half_size = i.bounds.zw * 0.5;
    float2 center = i.bounds.xy + half_size;
    float2 reach = covered_reach(i.xform, half_size);
    float2 lo = max(center - reach - 1.0, i.clip.xy - 1.0);
    float2 hi = min(center + reach + 1.0, i.clip.xy + i.clip.zw + 1.0);
    hi = max(hi, lo);
    o.position = to_clip(lerp(lo, hi, corner));
    o.bounds = i.bounds;
    o.clip = i.clip;
    o.shape = i.shape;
    o.fill = i.fill;
    o.border = i.border;
    o.gradient = i.gradient;
    o.stop_a = i.stop_a;
    o.stop_b = i.stop_b;
    o.stop_offsets = i.stop_offsets;
    o.xform = i.xform;
    return o;
}

float inner_distance(float2 p, float4 bounds, float radius, float edge, float outer, float scale)
{
    float2 half_size = bounds.zw * 0.5;
    float2 relative = p - bounds.xy - half_size;
    float2 offset = abs(relative) - half_size;
    float2 corner = offset + radius;
    float2 straight = offset + edge;
    if (corner.x <= 0.0 || corner.y <= 0.0)
    {
        return -max(straight.x, straight.y) * scale;
    }
    if (straight.x > 0.0 || straight.y > 0.0)
    {
        return -1.0;
    }
    return -(outer + edge * scale);
}

float3 oklab_to_linear(float3 lab)
{
    float l = lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z;
    float m = lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z;
    float s = lab.x - 0.0894841775 * lab.y - 1.2914855480 * lab.z;
    l = l * l * l;
    m = m * m * m;
    s = s * s * s;
    return float3(
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s);
}

float4 gradient_fill(QuadOut i, float2 p)
{
    float2 axis = i.gradient.zw - i.gradient.xy;
    float length_squared = max(dot(axis, axis), 1.0e-12);
    float t = saturate(dot(p - i.gradient.xy, axis) / length_squared);
    float span = max(i.stop_offsets.y - i.stop_offsets.x, 1.0e-6);
    float u = saturate((t - i.stop_offsets.x) / span);
    float4 lab = lerp(i.stop_a, i.stop_b, u);
    float3 linear_rgb = saturate(oklab_to_linear(lab.xyz));
    float3 encoded = float3(srgb_encode(linear_rgb.r), srgb_encode(linear_rgb.g), srgb_encode(linear_rgb.b));
    return float4(encoded * lab.w, lab.w);
}

float4 ps_quad(QuadOut i) : SV_Target
{
    float2 p = i.position.xy;
    float2 q = to_local(i.xform, i.bounds.xy + i.bounds.zw * 0.5, p);
    float scale = is_identity(i.xform) ? 1.0 : xform_scale(i.xform);
    float radius = i.shape.x;
    float border_width = i.shape.y;
    float edge = border_width == 0.0 ? -0.5 : border_width;
    float outer = box_distance(q, i.bounds, radius) * scale;
    float inner = inner_distance(q, i.bounds, radius, edge, outer, scale);
    float4 fill = i.shape.w > 0.5 ? gradient_fill(i, q) : i.fill;
    float4 color = fill;
    if (max(inner, outer) < 0.5)
    {
        float alpha = i.border.a;
        float4 blended = float4(i.border.rgb * alpha + fill.rgb * (1.0 - alpha), alpha + fill.a * (1.0 - alpha));
        color = lerp(fill, blended, saturate(0.5 - inner));
    }
    float clip_cover = saturate(0.5 - box_distance(p, i.clip, 0.0));
    return color * (saturate(0.5 - outer) * clip_cover * i.shape.z);
}
