float text_luminance(float4 premultiplied)
{
    float3 straight = premultiplied.a > 0.0 ? premultiplied.rgb / premultiplied.a : float3(0.0, 0.0, 0.0);
    return dot(straight, float3(0.30, 0.59, 0.11));
}

float tone_alpha(float coverage, float luminance)
{
    float contrast = values.x;
    float boost = contrast * saturate(4.0 * (0.75 - luminance));
    float alpha = coverage * (boost + 1.0) / (coverage * boost + 1.0);
    float level = region.x * luminance + region.y;
    float offset = region.z * luminance + region.w;
    return saturate(alpha + alpha * (1.0 - alpha) * (level * alpha + offset));
}
