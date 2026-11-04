#include "common.hlsli"

float4 tap(float2 position, float2 low, float2 high)
{
    float2 clamped = clamp(position, low + 0.5, max(high - 0.5, low + 0.5));
    return source.SampleLevel(linear_clamp, clamped / source_size, 0);
}

float4 ps_blur_down(float4 position : SV_Position) : SV_Target
{
    float2 ratio = region.zw / viewport;
    float2 center = region.xy + position.xy * ratio;
    float2 reach = 0.5 * values.x * max(ratio, 1.0);
    float2 low = max(region.xy, 0.0);
    float2 high = min(region.xy + region.zw, source_size);
    float4 sum = tap(center, low, high) * 4.0;
    sum += tap(center + float2(-reach.x, -reach.y), low, high);
    sum += tap(center + float2(reach.x, -reach.y), low, high);
    sum += tap(center + float2(-reach.x, reach.y), low, high);
    sum += tap(center + float2(reach.x, reach.y), low, high);
    return sum / 8.0;
}

float4 ps_blur_up(float4 position : SV_Position) : SV_Target
{
    float2 center = position.xy * (source_size / viewport);
    float reach = 0.5 * values.x;
    float2 low = float2(0.0, 0.0);
    float2 high = source_size;
    float4 sum = tap(center + float2(-2.0 * reach, 0.0), low, high);
    sum += tap(center + float2(2.0 * reach, 0.0), low, high);
    sum += tap(center + float2(0.0, -2.0 * reach), low, high);
    sum += tap(center + float2(0.0, 2.0 * reach), low, high);
    sum += tap(center + float2(-reach, -reach), low, high) * 2.0;
    sum += tap(center + float2(reach, -reach), low, high) * 2.0;
    sum += tap(center + float2(-reach, reach), low, high) * 2.0;
    sum += tap(center + float2(reach, reach), low, high) * 2.0;
    return sum / 12.0;
}
