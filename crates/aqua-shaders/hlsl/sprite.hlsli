#include "common.hlsli"

float clip_cover(float2 p, float4 clip)
{
    return saturate(0.5 - box_distance(p, clip, 0.0));
}
