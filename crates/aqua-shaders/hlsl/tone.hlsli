float srgb_encode(float value)
{
    float v = saturate(value);
    return v <= 0.0031308 ? v * 12.92 : 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

float text_luminance(float4 premultiplied)
{
    float3 straight = premultiplied.a > 0.0 ? premultiplied.rgb / premultiplied.a : float3(0.0, 0.0, 0.0);
    float3 encoded = float3(srgb_encode(straight.r), srgb_encode(straight.g), srgb_encode(straight.b));
    return dot(encoded, float3(0.30, 0.59, 0.11));
}

float tone_adjust(float coverage, float luminance)
{
    float contrast = values.x;
    float boost = contrast * saturate(4.0 * (0.75 - luminance));
    float alpha = coverage * (boost + 1.0) / (coverage * boost + 1.0);
    float level = region.x * luminance + region.y;
    float offset = region.z * luminance + region.w;
    return alpha + alpha * (1.0 - alpha) * (level * alpha + offset);
}

float srgb_decode(float value)
{
    float v = saturate(value);
    return v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4);
}

float linear_blend_alpha(float coverage, float luminance, float known_backdrop)
{
    float toned = saturate(tone_adjust(coverage, luminance));
    float backdrop = known_backdrop >= 0.0 ? known_backdrop : (luminance > 0.5 ? 0.0 : 1.0);
    float encoded = backdrop + (luminance - backdrop) * toned;
    float text = srgb_decode(luminance);
    float behind = srgb_decode(backdrop);
    float blended = srgb_decode(encoded);
    return abs(text - behind) > 1.0e-4 ? saturate((blended - behind) / (text - behind)) : toned;
}
