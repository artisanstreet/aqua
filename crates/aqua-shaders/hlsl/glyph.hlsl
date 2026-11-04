#include "sprite.hlsli"

#ifdef BINDLESS
struct BoundAtlas
{
    float Load(int3 location)
    {
        Texture2D<float> bound = ResourceDescriptorHeap[texture_index];
        return bound.Load(location);
    }
};

static BoundAtlas atlas;
#else
Texture2D<float> atlas : register(t0);
#endif

struct GlyphIn
{
    float4 bounds : BOUNDS;
    float4 texels : TEXELS;
    float4 clip : CLIP;
    float4 color : COLOR;
    uint vertex : SV_VertexID;
};

struct GlyphOut
{
    float4 position : SV_Position;
    nointerpolation float4 bounds : BOUNDS;
    nointerpolation float4 texels : TEXELS;
    nointerpolation float4 clip : CLIP;
    nointerpolation float4 color : COLOR;
};

float srgb_encode(float value)
{
    float v = saturate(value);
    return v <= 0.0031308 ? v * 12.92 : 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

float text_luminance(float4 premultiplied)
{
    float3 straight = premultiplied.a > 0.0 ? premultiplied.rgb / premultiplied.a : float3(0.0, 0.0, 0.0);
    float3 encoded = float3(srgb_encode(straight.r), srgb_encode(straight.g), srgb_encode(straight.b));
    return dot(encoded, float3(0.30, 0.59, 0.11));
}

float tone_adjust(float coverage, float luminance)
{
    float contrast = values.x;
    float boost = contrast * saturate(4.0 * (0.75 - luminance));
    float alpha = coverage * (boost + 1.0) / (coverage * boost + 1.0);
    float level = region.x * luminance + region.y;
    float offset = region.z * luminance + region.w;
    return alpha + alpha * (1.0 - alpha) * (level * alpha + offset);
}

float srgb_decode(float value)
{
    float v = saturate(value);
    return v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4);
}

float linear_blend_alpha(float coverage, float luminance)
{
    float toned = saturate(tone_adjust(coverage, luminance));
    float backdrop = luminance > 0.5 ? 0.0 : 1.0;
    float encoded = backdrop + (luminance - backdrop) * toned;
    float text = srgb_decode(luminance);
    float behind = srgb_decode(backdrop);
    float blended = srgb_decode(encoded);
    return abs(text - behind) > 1.0e-4 ? saturate((blended - behind) / (text - behind)) : toned;
}

GlyphOut vs_glyph(GlyphIn i)
{
    GlyphOut o;
    float2 corner = float2(i.vertex & 1, i.vertex >> 1);
    o.position = to_clip(i.bounds.xy + corner * i.bounds.zw);
    o.bounds = i.bounds;
    o.texels = i.texels;
    o.clip = i.clip;
    o.color = i.color;
    return o;
}

float4 ps_glyph(GlyphOut i) : SV_Target
{
    float2 p = i.position.xy;
    float2 scale = i.texels.zw / max(i.bounds.zw, float2(1.0e-4, 1.0e-4));
    float2 local = floor((p - i.bounds.xy) * scale);
    local = clamp(local, float2(0.0, 0.0), i.texels.zw - 1.0);
    float coverage = atlas.Load(int3(int2(i.texels.xy + local), 0));
    float toned = linear_blend_alpha(coverage, text_luminance(i.color));
    return i.color * (toned * clip_cover(p, i.clip));
}
