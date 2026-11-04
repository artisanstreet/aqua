Texture2D<float4> input0 : register(t0);
RWTexture2D<float4> output : register(u0);

cbuffer Parameters : register(b0)
{
    float4 parameters;
};

[numthreads(64, 1, 1)]
void cs_row_box_blur(uint3 id : SV_DispatchThreadID)
{
    uint width;
    uint height;
    output.GetDimensions(width, height);
    if (id.x >= width || id.y >= height)
    {
        return;
    }
    int reach = (int)floor(max(parameters.x, 0.0) + 0.5);
    float4 sum = float4(0.0, 0.0, 0.0, 0.0);
    for (int offset = -reach; offset <= reach; ++offset)
    {
        int tap = clamp((int)id.x + offset, 0, (int)width - 1);
        sum += input0.Load(int3(tap, (int)id.y, 0));
    }
    output[id.xy] = sum / (float)(2 * reach + 1);
}
