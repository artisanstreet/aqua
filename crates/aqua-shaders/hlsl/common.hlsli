cbuffer Frame : register(b0)
{
    float2 viewport;
    float opacity;
    float frame_pad;
    float4 region;
    float4 values;
    float2 source_size;
    float2 frame_pad2;
};

Texture2D<float4> source : register(t0);
SamplerState linear_clamp : register(s0);

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
