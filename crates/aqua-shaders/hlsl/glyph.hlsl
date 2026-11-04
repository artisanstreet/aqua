#include "sprite.hlsli"

Texture2D<float> atlas : register(t0);

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
    return i.color * (coverage * clip_cover(p, i.clip));
}
