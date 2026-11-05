cbuffer Frame : register(b0)
{
    float2 viewport;
    float opacity;
    float frame_pad;
    float4 region;
    float4 values;
    float2 source_size;
    uint texture_index;
    uint frame_pad2;
    float4 matrix_rows[5];
};

#ifdef BINDLESS
struct BoundSource
{
    float4 Load(int3 location)
    {
        Texture2D<float4> bound = ResourceDescriptorHeap[texture_index];
        return bound.Load(location);
    }

    float4 SampleLevel(SamplerState state, float2 uv, float level)
    {
        Texture2D<float4> bound = ResourceDescriptorHeap[texture_index];
        return bound.SampleLevel(state, uv, level);
    }
};

static BoundSource source;
#else
Texture2D<float4> source : register(t0);
#endif
SamplerState linear_clamp : register(s0);

float srgb_encode(float value)
{
    float v = saturate(value);
    return v <= 0.0031308 ? v * 12.92 : 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

float4 to_clip(float2 pixel)
{
    return float4(pixel.x / viewport.x * 2.0 - 1.0, 1.0 - pixel.y / viewport.y * 2.0, 0.0, 1.0);
}

float box_distance(float2 p, float4 bounds, float radius)
{
    float2 half_size = bounds.zw * 0.5;
    float2 relative = p - bounds.xy - half_size;
    float2 q = abs(relative) - half_size + radius;
    if (radius == 0.0)
    {
        return max(q.x, q.y);
    }
    return length(max(q, float2(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

float4 vs_fullscreen(uint vertex : SV_VertexID) : SV_Position
{
    float2 corner = float2(vertex & 1, vertex >> 1);
    return float4(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
}

bool is_identity(float4 xform)
{
    return xform.x == 1.0 && xform.y == 0.0 && xform.z == 0.0 && xform.w == 1.0;
}

float xform_scale(float4 xform)
{
    return sqrt(abs(xform.x * xform.w - xform.y * xform.z));
}

float2 xform_apply(float4 xform, float2 v)
{
    return float2(xform.x * v.x + xform.z * v.y, xform.y * v.x + xform.w * v.y);
}

float4 xform_inverse(float4 xform)
{
    float det = xform.x * xform.w - xform.y * xform.z;
    float reciprocal = 1.0 / (abs(det) > 1.0e-12 ? det : 1.0);
    return float4(xform.w * reciprocal, -xform.y * reciprocal, -xform.z * reciprocal, xform.x * reciprocal);
}

float2 to_local(float4 xform, float2 center, float2 p)
{
    if (is_identity(xform))
    {
        return p;
    }
    return center + xform_apply(xform_inverse(xform), p - center);
}

float2 covered_reach(float4 xform, float2 half_size)
{
    return float2(
        abs(xform.x) * half_size.x + abs(xform.z) * half_size.y,
        abs(xform.y) * half_size.x + abs(xform.w) * half_size.y);
}
