#include "common.hlsli"

float coefficient(uint index)
{
    return matrix_rows[index >> 2][index & 3];
}

float4 ps_color_matrix(float4 position : SV_Position) : SV_Target
{
    float4 pixel = source.Load(int3(int2(position.xy), 0));
    float3 straight = pixel.a > 0.0 ? pixel.rgb / pixel.a : float3(0.0, 0.0, 0.0);
    float4 input = float4(straight, pixel.a);
    float4 result;
    [unroll]
    for (uint row = 0; row < 4; ++row)
    {
        uint base = row * 5;
        float4 weights = float4(coefficient(base), coefficient(base + 1), coefficient(base + 2), coefficient(base + 3));
        result[row] = saturate(dot(weights, input) + coefficient(base + 4));
    }
    return float4(result.rgb * result.a, result.a);
}
