cbuffer Parameters : register(b0) { float4 parameters[9]; };
struct Input { float4 position : SV_Position; float2 uv : TEXCOORD0; };

float3 srgb(float3 linear_rgb) {
    float3 low = linear_rgb * 12.92;
    float3 high = 1.055 * pow(max(linear_rgb, 0.0), 1.0 / 2.4) - 0.055;
    return float3(linear_rgb.r <= 0.0031308 ? low.r : high.r,
        linear_rgb.g <= 0.0031308 ? low.g : high.g,
        linear_rgb.b <= 0.0031308 ? low.b : high.b);
}

float3 oklab(float3 lch) {
    float hue = radians(lch.z);
    return float3(lch.x, lch.y * cos(hue), lch.y * sin(hue));
}

float3 lab_srgb(float3 lab) {
    float3 lms = float3(lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z,
        lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z,
        lab.x - 0.0894841775 * lab.y - 1.2914855480 * lab.z);
    lms = lms * lms * lms;
    float3 linear_rgb = float3(4.0767416621 * lms.x - 3.3077115913 * lms.y + 0.2309699292 * lms.z,
        -1.2684380046 * lms.x + 2.6097574011 * lms.y - 0.3413193965 * lms.z,
        -0.0041960863 * lms.x - 0.7034186147 * lms.y + 1.7076147010 * lms.z);
    return srgb(max(linear_rgb, 0.0));
}

float3 gradient_segment(float3 a, float3 b, float3 slope_a, float3 slope_b, float width, float position) {
    float t = clamp(position / width, 0.0, 1.0);
    float t2 = t * t;
    float t3 = t2 * t;
    return (2.0 * t3 - 3.0 * t2 + 1.0) * a
        + (t3 - 2.0 * t2 + t) * width * slope_a
        + (-2.0 * t3 + 3.0 * t2) * b
        + (t3 - t2) * width * slope_b;
}

float3 sky_color(float altitude) {
    float3 p0 = parameters[2].xyz;
    float3 p1 = parameters[3].xyz;
    float3 p2 = parameters[4].xyz;
    float3 p3 = parameters[5].xyz;
    float3 p4 = parameters[6].xyz;
    float3 m0 = (p1 - p0) / 0.24;
    float3 m1 = (p2 - p0) / 0.46;
    float3 m2 = (p3 - p1) / 0.44;
    float3 m3 = (p4 - p2) / 0.42;
    float3 m4 = (p4 - p3) / 0.20;
    float3 lab = altitude < 0.24
        ? gradient_segment(p0, p1, m0, m1, 0.24, altitude)
        : altitude < 0.46
            ? gradient_segment(p1, p2, m1, m2, 0.22, altitude - 0.24)
            : altitude < 0.68
                ? gradient_segment(p2, p3, m2, m3, 0.22, altitude - 0.46)
                : gradient_segment(p3, p4, m3, m4, 0.20, altitude - 0.68);
    return lab_srgb(lab);
}

float screen_noise(float2 p) {
    return frac(52.9829189 * frac(dot(p, float2(0.06711056, 0.00583715))));
}

float hash(float2 p) {
    float3 q = frac(float3(p.x, p.y, p.x) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return frac((q.x + q.y) * q.z);
}

float noise(float2 p) {
    float2 cell = floor(p);
    float2 f = frac(p);
    f = f * f * (3.0 - 2.0 * f);
    return lerp(lerp(hash(cell), hash(cell + float2(1.0, 0.0)), f.x),
        lerp(hash(cell + float2(0.0, 1.0)), hash(cell + float2(1.0, 1.0)), f.x), f.y);
}

float4 winter_sky(Input input) : SV_Target {
    float2 uv = input.uv;
    float3 sunlight = parameters[7].rgb;
    float2 size = parameters[0].xy;
    float time = parameters[0].z;
    float unit = min(size.x, size.y * 1.4);
    float x = (uv.x - 0.5) * size.x / unit;
    float y = (uv.y - parameters[8].x) * size.y / unit;
    float altitude = 0.16 * x * x - y;
    float pixel = 1.0 / unit;
    float3 background = parameters[1].rgb;
    float above = smoothstep(-pixel, pixel * 1.5, altitude);
    float gradient = 0.88 - max(altitude, 0.0) * 2.2 + sin(time * 0.03) * 0.002;
    float3 color = background;
    [branch] if (altitude > -pixel) {
        color = lerp(background, sky_color(gradient), above);
    }
    [branch] if (abs(altitude) < 0.06) {
        float burn_time = time * 0.55;
        float coarse = noise(float2(x * 22.0 - burn_time * 0.3, burn_time * 0.4));
        float serration = noise(float2(x * 100.0 - burn_time * 0.5, burn_time * 1.4));
        float chatter = noise(float2(x * 270.0 + burn_time * 0.8, burn_time * 3.7));
        float warp = (coarse - 0.5) * 0.0012 + (chatter - 0.5) * 0.0006;
        float distance_to_rim = altitude + warp;
        float reach = 0.0010 + serration * serration * serration * 0.005;
        float flame = noise(float2(x * 180.0 + coarse - 0.5, altitude * 180.0 - burn_time * 2.5));
        float burn = exp(-abs(distance_to_rim) / reach) * smoothstep(0.25, 0.85, flame)
            * (1.0 - smoothstep(0.004, 0.009, distance_to_rim))
            * smoothstep(-0.003, 0.001, distance_to_rim);
        float rim = exp(-abs(distance_to_rim) * 1000.0) * (0.85 + chatter * 0.12)
            * (0.95 + serration * 0.10);
        float glow = exp(-abs(distance_to_rim) * 150.0) * 0.20;
        float sun = exp(-x * x * 35.0) * exp(-abs(distance_to_rim) * 140.0) * 0.10;
        color += sunlight * (rim * 0.70 + burn * 0.40 + glow * 0.40 + sun);
        color += sky_color(0.68) * (glow * 0.40 + burn * 0.50);
    }
    float edge = smoothstep(0.0, 0.13, uv.x) * smoothstep(0.0, 0.13, 1.0 - uv.x)
        * smoothstep(0.0, 0.12, uv.y) * smoothstep(0.0, 0.16, 1.0 - uv.y);
    color = lerp(parameters[1].rgb, color, edge);
    float2 screen_pixel = floor(float2(uv.x * size.x, (1.0 - uv.y) * size.y));
    float3 dither = float3(screen_noise(screen_pixel),
        screen_noise(screen_pixel + float2(17.0, 59.0)),
        screen_noise(screen_pixel + float2(43.0, 11.0))) - 0.5;
    float edge_pixel = min(min(uv.x, 1.0 - uv.x) * size.x, min(uv.y, 1.0 - uv.y) * size.y);
    color = max(color + dither / 255.0 * smoothstep(0.0, 1.0, edge_pixel), 0.0);
    float radius = parameters[7].w * parameters[0].w;
    float2 corner = abs((uv - 0.5) * size) - size * 0.5 + radius;
    float distance = length(max(corner, 0.0)) + min(max(corner.x, corner.y), 0.0) - radius;
    float coverage = clamp(0.5 - distance, 0.0, 1.0);
    return float4(color * coverage, coverage);
}
