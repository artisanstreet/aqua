cbuffer Parameters : register(b0) { float4 parameters[7]; };
struct Input { float4 position : SV_Position; float2 uv : TEXCOORD0; };

float hash(float2 p) {
    float3 q = frac(float3(p.x, p.y, p.x) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return frac((q.x + q.y) * q.z);
}

float noise(float2 p) {
    float2 cell = floor(p);
    float2 f = frac(p);
    f = f * f * (3.0 - 2.0 * f);
    return lerp(lerp(hash(cell), hash(cell + float2(1, 0)), f.x),
                lerp(hash(cell + float2(0, 1)), hash(cell + float2(1, 1)), f.x), f.y);
}

float turbulence(float2 p) {
    return noise(p) * 0.57 + noise(p * 2.03 + 7.1) * 0.28
        + noise(p * 4.11 + 19.7) * 0.15;
}

float4 solar_horizon(Input input) : SV_Target {
    float2 size = parameters[0].xy;
    float time = parameters[0].z * 0.08;
    float burn_time = parameters[0].z * 0.7;
    float unit = min(size.x, size.y * 1.4);
    float x = (input.uv.x - 0.5) * size.x / unit;
    float y = (input.uv.y - parameters[5].x) * size.y / unit;
    float altitude = 0.16 * x * x - y;
    float edge_fade = smoothstep(0.0, 0.13, input.uv.x)
        * smoothstep(0.0, 0.13, 1.0 - input.uv.x)
        * smoothstep(0.0, 0.12, input.uv.y)
        * smoothstep(0.0, 0.16, 1.0 - input.uv.y);
    float above = smoothstep(-0.002, 0.003, altitude);
    float coarse = turbulence(float2(x * 20.0 + time * 0.25, time * 0.55));
    float serration = turbulence(float2(x * 75.0 - burn_time * 0.6, burn_time * 1.6));
    float chatter = noise(float2(x * 260.0 - burn_time * 2.0, burn_time * 5.5));
    float warp = (coarse - 0.5) * 0.0015 + (chatter - 0.5) * 0.00065;
    float drift = sin(x * 12.0 - time * 0.9) * 0.0005;
    float distance = altitude + warp + drift;
    float flame = turbulence(float2(x * 130.0 + (coarse - 0.5) * 2.0, altitude * 260.0 - burn_time * 2.4));
    float reach = 0.0012 + pow(serration, 3.0) * 0.007;
    float fringe = exp(-abs(distance) / reach) * pow(saturate(flame * 1.9 - 0.45), 1.3);
    float tearing = saturate((serration - 0.35) * 2.0);
    float burn_mask = (1.0 - smoothstep(0.004, 0.010, distance))
        * smoothstep(-0.004, 0.001, distance);
    float plasma = fringe * (0.45 + tearing) * burn_mask;
    float surge = noise(float2(x * 26.0 + burn_time * 0.35, burn_time * 0.8));
    float pulse = 0.86 + surge * 0.22 + sin(burn_time * 1.7 + x * 8.0) * 0.04;
    float current = turbulence(float2(x * 7.0 - time * 1.3, altitude * 12.0 + time * 0.6));
    float heat = noise(float2(x * 12.0 - time * 1.4, time * 0.35));
    float atmosphere_height = 0.13 + (current - 0.5) * 0.026;
    float blue = exp(-pow((altitude - atmosphere_height) / 0.14, 2.0))
        * above * 0.52 * (0.92 + current * 0.16);
    float haze = exp(-max(altitude, 0.0) * 38.0) * above * 0.38 * (0.94 + heat * 0.12);
    float glow = exp(-abs(distance) * 110.0) * (0.38 + serration * 0.04 + heat * 0.12) * (0.94 + surge * 0.12);
    float rim = exp(-abs(distance) * 1000.0) * (0.82 + chatter * 0.14) * pulse;
    float filament = exp(-abs(distance - (flame - 0.5) * 0.0015) * 1100.0);
    float3 background = parameters[1].rgb;
    float3 color = background;
    color += parameters[2].rgb * blue;
    float red_note = smoothstep(0.38, 0.72, noise(float2(x * 9.0 + 4.0 - time * 0.7, time * 0.2))) * 0.44;
    float3 ember = lerp(parameters[3].rgb, parameters[6].rgb, red_note);
    color += ember * (haze + glow + plasma * 0.62);
    color += parameters[4].rgb * (rim * 0.50 + filament * 0.08 + plasma * 0.06);
    float grain = (hash(floor(input.uv * size)) - 0.5) * 0.055;
    color += grain * sqrt(max(color - background, 0.0));
    color = lerp(background, max(color, 0.0), edge_fade);
    float radius = parameters[5].y * parameters[0].w;
    float2 corner = abs((input.uv - 0.5) * size) - size * 0.5 + radius;
    float edge_distance = length(max(corner, 0.0)) + min(max(corner.x, corner.y), 0.0) - radius;
    float coverage = saturate(0.5 - edge_distance);
    return float4(color * coverage, coverage);
}
