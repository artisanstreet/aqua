#include "sprite.hlsli"

#ifdef BINDLESS
struct BoundPage
{
    float4 Load(int3 location)
    {
        Texture2D<float4> bound = ResourceDescriptorHeap[texture_index];
        return bound.Load(location);
    }
};

static BoundPage page;
#else
Texture2D<float4> page : register(t0);
#endif

struct ImageIn
{
    float4 bounds : BOUNDS;
    float4 texels : TEXELS;
    float4 clip : CLIP;
    float4 params : PARAMS;
    uint vertex : SV_VertexID;
};

struct ImageOut
{
    float4 position : SV_Position;
    nointerpolation float4 bounds : BOUNDS;
    nointerpolation float4 texels : TEXELS;
    nointerpolation float4 clip : CLIP;
    nointerpolation float4 params : PARAMS;
};

ImageOut vs_image(ImageIn i)
{
    ImageOut o;
    float2 corner = float2(i.vertex & 1, i.vertex >> 1);
    float2 lo = max(i.bounds.xy - 1.0, i.clip.xy - 1.0);
    float2 hi = min(i.bounds.xy + i.bounds.zw + 1.0, i.clip.xy + i.clip.zw + 1.0);
    hi = max(hi, lo);
    o.position = to_clip(lerp(lo, hi, corner));
    o.bounds = i.bounds;
    o.texels = i.texels;
    o.clip = i.clip;
    o.params = i.params;
    return o;
}

float4 fetch(float2 texel, float4 region)
{
    float2 clamped = clamp(texel, region.xy, region.xy + region.zw - 1.0);
    return page.Load(int3(int2(clamped), 0));
}

struct ReplaceOut
{
    float4 color : SV_Target0;
    float4 weight : SV_Target1;
};

float4 shade_image(ImageOut i, out float weight)
{
    float2 p = i.position.xy;
    float radius = i.params.x;
    float opacity_factor = i.params.y;
    float clip_radius = i.params.z;
    float cover = saturate(0.5 - box_distance(p, i.bounds, radius))
        * saturate(0.5 - box_distance(p, i.clip, clip_radius));
    float2 uv = (p - i.bounds.xy) / max(i.bounds.zw, float2(1.0e-4, 1.0e-4));
    float2 position = i.texels.xy + uv * i.texels.zw;
    float2 shifted = position - 0.5;
    float2 base = floor(shifted);
    float2 f = shifted - base;
    float4 a = fetch(base, i.texels);
    float4 b = fetch(base + float2(1.0, 0.0), i.texels);
    float4 c = fetch(base + float2(0.0, 1.0), i.texels);
    float4 d = fetch(base + float2(1.0, 1.0), i.texels);
    float4 color = lerp(lerp(a, b, f.x), lerp(c, d, f.x), f.y);
    weight = cover * opacity_factor;
    return color;
}

float4 ps_image(ImageOut i) : SV_Target
{
    float weight;
    float4 color = shade_image(i, weight);
    return color * weight;
}

ReplaceOut ps_image_replace(ImageOut i)
{
    float weight;
    ReplaceOut o;
    float4 color = shade_image(i, weight);
    o.color = color * weight;
    o.weight = float4(weight, weight, weight, weight);
    return o;
}
