#include "common.hlsli"

static const float HARD_EDGE_SIGMA = 0.05;

struct ShadowIn
{
    float4 bounds : BOUNDS;
    float4 clip : CLIP;
    float4 shape : SHAPE;
    float4 offset : OFFSET;
    float4 color : COLOR;
    uint vertex : SV_VertexID;
};

struct ShadowOut
{
    float4 position : SV_Position;
    nointerpolation float4 bounds : BOUNDS;
    nointerpolation float4 clip : CLIP;
    nointerpolation float4 shape : SHAPE;
    nointerpolation float4 offset : OFFSET;
    nointerpolation float4 color : COLOR;
};

ShadowOut vs_shadow(ShadowIn i)
{
    ShadowOut o;
    float2 corner = float2(i.vertex & 1, i.vertex >> 1);
    float reach = max(i.shape.z, 0.0) + 3.0 * max(i.shape.y, 0.0) + 1.0;
    float2 lo = max(i.bounds.xy + i.offset.xy - reach, i.clip.xy - 1.0);
    float2 hi = min(i.bounds.xy + i.bounds.zw + i.offset.xy + reach, i.clip.xy + i.clip.zw + 1.0);
    hi = max(hi, lo);
    o.position = to_clip(lerp(lo, hi, corner));
    o.bounds = i.bounds;
    o.clip = i.clip;
    o.shape = i.shape;
    o.offset = i.offset;
    o.color = i.color;
    return o;
}

float2 error_function(float2 value)
{
    float2 s = float2(value.x < 0.0 ? -1.0 : 1.0, value.y < 0.0 ? -1.0 : 1.0);
    float2 a = abs(value);
    float2 x = 1.0 + (0.278393 + (0.230389 + 0.078108 * a * a) * a) * a;
    x *= x;
    return s - s / (x * x);
}

float blur_along_x(float x, float y, float sigma, float corner, float2 half_size)
{
    float delta = min(half_size.y - corner - abs(y), 0.0);
    float curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    float2 integral = 0.5 + 0.5 * error_function((float2(x, x) + float2(-curved, curved)) * (0.70710678118 / sigma));
    return integral.y - integral.x;
}


float shadow_alpha(float2 p, float4 rect, float radius, float sigma)
{
    float2 half_size = rect.zw * 0.5;
    float2 relative = p - rect.xy - half_size;
    if (sigma < HARD_EDGE_SIGMA)
    {
        return saturate(0.5 - box_distance(p, rect, min(radius, min(half_size.x, half_size.y))));
    }
    float low = relative.y - half_size.y;
    float high = relative.y + half_size.y;
    float start = clamp(-3.0 * sigma, low, high);
    float stop = clamp(3.0 * sigma, low, high);
    float step_size = (stop - start) / 4.0;
    float y = start + step_size * 0.5;
    float alpha = 0.0;
    [unroll]
    for (int n = 0; n < 4; n++)
    {
        float gaussian = exp(-(y * y) / (2.0 * sigma * sigma)) / (2.50662827463 * sigma);
        alpha += blur_along_x(relative.x, relative.y - y, sigma, radius, half_size) * gaussian * step_size;
        y += step_size;
    }
    return alpha;
}

float4 ps_shadow(ShadowOut i) : SV_Target
{
    float2 p = i.position.xy;
    float spread = i.shape.z;
    float2 size = max(i.bounds.zw + 2.0 * spread, 0.0);
    float2 center = i.bounds.xy + i.bounds.zw * 0.5 + i.offset.xy;
    float4 rect = float4(center - size * 0.5, size);
    float radius = max(i.shape.x + spread, 0.0);
    float alpha = shadow_alpha(p, rect, radius, i.shape.y);
    float clip_cover = saturate(0.5 - box_distance(p, i.clip, 0.0));
    return i.color * (alpha * clip_cover * i.shape.w);
}
