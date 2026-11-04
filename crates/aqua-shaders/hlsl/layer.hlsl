#include "common.hlsli"

float4 ps_layer(float4 position : SV_Position) : SV_Target
{
    return source.Load(int3(int2(position.xy), 0)) * opacity;
}

float4 ps_copy(float4 position : SV_Position) : SV_Target
{
    return source.Load(int3(int2(position.xy + region.xy), 0));
}
