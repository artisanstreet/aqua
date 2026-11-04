#include "sprite.hlsli"
#include "tone.hlsli"

#ifdef BINDLESS
struct BoundStore
{
    float4 Load(int3 location)
    {
        Texture2D<float4> bound = ResourceDescriptorHeap[texture_index];
        return bound.Load(location);
    }
};

static BoundStore store;
#else
Texture2D<float4> store : register(t0);
#endif

static const uint STORE_SHIFT = 10;
static const uint STORE_MASK = 1023;

struct SlugIn
{
    float4 placement : PLACEMENT;
    float4 glyph : GLYPH;
    float4 bounds : EMBOUNDS;
    float4 clip : CLIP;
    float4 color : COLOR;
    uint vertex : SV_VertexID;
};

struct SlugOut
{
    float4 position : SV_Position;
    nointerpolation float4 placement : PLACEMENT;
    nointerpolation float4 glyph : GLYPH;
    nointerpolation float4 bounds : EMBOUNDS;
    nointerpolation float4 clip : CLIP;
    nointerpolation float4 color : COLOR;
};

float4 texel(uint index)
{
    return store.Load(int3(index & STORE_MASK, index >> STORE_SHIFT, 0));
}

float2 em_to_screen(SlugIn i, float2 em)
{
    float4 xform = float4(i.placement.zw, i.glyph.xy);
    return i.placement.xy + xform_apply(xform, em);
}

SlugOut vs_slug(SlugIn i)
{
    SlugOut o;
    float4 xform = float4(i.placement.zw, i.glyph.xy);
    float2 corner = float2(i.vertex & 1, i.vertex >> 1);
    float2 a = em_to_screen(i, i.bounds.xy);
    float2 b = em_to_screen(i, float2(i.bounds.z, i.bounds.y));
    float2 c = em_to_screen(i, float2(i.bounds.x, i.bounds.w));
    float2 d = em_to_screen(i, i.bounds.zw);
    float2 lo = min(min(a, b), min(c, d)) - 1.0;
    float2 hi = max(max(a, b), max(c, d)) + 1.0;
    lo = max(lo, i.clip.xy - 1.0);
    hi = min(hi, i.clip.xy + i.clip.zw + 1.0);
    hi = max(hi, lo);
    o.position = to_clip(lerp(lo, hi, corner));
    o.placement = i.placement;
    o.glyph = i.glyph;
    o.bounds = i.bounds;
    o.clip = i.clip;
    o.color = i.color;
    return o;
}

uint root_code(float y1, float y2, float y3)
{
    uint shift = (asuint(y1) >> 31) | ((asuint(y2) >> 31) << 1) | ((asuint(y3) >> 31) << 2);
    return (0x2E74u >> shift) & 0x0101u;
}

float2 solve_axis(float3 along, float3 across)
{
    float a_across = along.x - 2.0 * along.y + along.z;
    float b_across = along.x - along.y;
    float t1;
    float t2;
    if (abs(a_across) < 1.0 / 65536.0)
    {
        float t = along.x / (2.0 * b_across);
        t1 = t;
        t2 = t;
    }
    else
    {
        float discriminant = sqrt(max(b_across * b_across - a_across * along.x, 0.0));
        float reciprocal = 1.0 / a_across;
        t1 = (b_across - discriminant) * reciprocal;
        t2 = (b_across + discriminant) * reciprocal;
    }
    float a = across.x - 2.0 * across.y + across.z;
    float b = across.x - across.y;
    return float2((a * t1 - b * 2.0) * t1 + across.x, (a * t2 - b * 2.0) * t2 + across.x);
}

float2 scan(uint glyph, float2 sample_point, float ppem, bool horizontal, out float weight)
{
    float4 header_a = texel(glyph + 1);
    float4 header_b = texel(glyph + 2);
    uint lists_at = uint(texel(glyph + 3).x);
    float origin = horizontal ? header_a.x : header_b.x;
    float per_em = horizontal ? header_a.y : header_b.y;
    uint count = uint(horizontal ? header_a.z : header_a.w);
    uint ranges_at = uint(horizontal ? header_b.z : header_b.w);
    float coordinate = horizontal ? sample_point.y : sample_point.x;
    float band_f = floor((coordinate - origin) * per_em);
    uint band = band_f < 0.0 ? 0u : min(uint(band_f), count - 1u);
    float4 range = texel(ranges_at + band);
    uint start = uint(range.x);
    uint members = uint(range.y);
    float coverage = 0.0;
    weight = 0.0;
    for (uint slot = start; slot < start + members; slot++)
    {
        float4 lane = texel(lists_at + (slot >> 2));
        uint lane_index = slot & 3u;
        uint curve_at = uint(lane_index == 0u ? lane.x : (lane_index == 1u ? lane.y : (lane_index == 2u ? lane.z : lane.w)));
        float4 first = texel(curve_at);
        float4 second = texel(curve_at + 1u);
        float2 p1 = first.xy - sample_point;
        float2 p2 = first.zw - sample_point;
        float2 p3 = second.xy - sample_point;
        float3 along = horizontal ? float3(p1.y, p2.y, p3.y) : float3(p1.x, p2.x, p3.x);
        float3 across = horizontal ? float3(p1.x, p2.x, p3.x) : float3(p1.y, p2.y, p3.y);
        float farthest = max(max(across.x, across.y), across.z);
        if (farthest * ppem < -0.5)
        {
            break;
        }
        uint code = root_code(along.x, along.y, along.z);
        if (code != 0u)
        {
            float2 roots = solve_axis(along, across) * ppem;
            if ((code & 1u) != 0u)
            {
                coverage += saturate(roots.x + 0.5);
                weight = max(weight, saturate(1.0 - abs(roots.x) * 2.0));
            }
            if (code > 1u)
            {
                coverage -= saturate(roots.y + 0.5);
                weight = max(weight, saturate(1.0 - abs(roots.y) * 2.0));
            }
        }
    }
    return float2(coverage, weight);
}

float slug_coverage(uint glyph, float2 sample_point, float ppem)
{
    float weight_x;
    float weight_y;
    float coverage_x = scan(glyph, sample_point, ppem, true, weight_x).x;
    float coverage_y = scan(glyph, sample_point, ppem, false, weight_y).x;
    float blended = abs(coverage_x * weight_x + coverage_y * weight_y) / max(weight_x + weight_y, 1.0 / 65536.0);
    return saturate(max(blended, min(abs(coverage_x), abs(coverage_y))));
}

float4 ps_slug(SlugOut i) : SV_Target
{
    float2 p = i.position.xy;
    float4 xform = float4(i.placement.zw, i.glyph.xy);
    float2 em = xform_apply(xform_inverse(xform), p - i.placement.xy);
    float ppem = xform_scale(xform);
    float margin = 1.0 / max(ppem, 1.0e-4);
    if (em.x < i.bounds.x - margin || em.x > i.bounds.z + margin || em.y < i.bounds.y - margin || em.y > i.bounds.w + margin)
    {
        return float4(0.0, 0.0, 0.0, 0.0);
    }
    float coverage = slug_coverage(uint(i.glyph.z), em, ppem);
    float toned = linear_blend_alpha(coverage, text_luminance(i.color), i.glyph.w);
    return i.color * (toned * clip_cover(p, i.clip));
}
