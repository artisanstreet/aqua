use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_core::scene::{GpuiSnapshotTransition, GpuiSourceSnapshotTransition, SnapshotTransition};
use aqua_gpu_ref::kernels::Kernels;
use aqua_gpu_ref::raster::Layer;
use aqua_graph::{CustomPassSpec, PaintCommand};
use aqua_protocol::Instant;

use super::color;

pub const NAME: &str = "aqua.snapshot_transition";
pub const ENTRY: &str = "transition";

pub const HLSL: &str = "Texture2D<float4> input0 : register(t0);\n\
cbuffer Parameters : register(b0) { float4 parameters[3]; };\n\
struct CustomInput { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
float4 transition(CustomInput input) : SV_Target {\n\
    float4 old = input0.Load(int3(input.position.xy, 0));\n\
    float2 viewport = parameters[0].xy;\n\
    float progress = parameters[0].z;\n\
    float band = parameters[0].w;\n\
    float4 tint = parameters[1];\n\
    float2 uv = input.position.xy / viewport;\n\
    float keep_old;\n\
    float strength;\n\
    if (parameters[2].x < 1.0) {\n\
        float diagonal = (uv.x + uv.y) * 0.5;\n\
        float edge = lerp(-band, 1.0 + band, progress);\n\
        float reveal = 1.0 - smoothstep(edge - band * 0.5, edge + band * 0.5, diagonal);\n\
        keep_old = 1.0 - reveal;\n\
        strength = (1.0 - abs(2.0 * reveal - 1.0)) * tint.a;\n\
    } else {\n\
        float t = (uv.x + uv.y) * 0.5;\n\
        float edge = -band + progress * (1.0 + 2.0 * band);\n\
        keep_old = smoothstep(edge - 0.008, edge + 0.008, t);\n\
        float offset = (t - edge) / max(band, 0.0001) + 0.5;\n\
        float lobe = saturate(min(offset, 1.0 - offset) / 0.4);\n\
        strength = tint.a * lobe * lobe * (3.0 - 2.0 * lobe);\n\
    }\n\
    float3 color = old.rgb * (1.0 - strength) * keep_old + tint.rgb * strength;\n\
    float alpha = 1.0 - (1.0 - strength) * (1.0 - keep_old);\n\
    return float4(color, alpha);\n\
}\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sweep {
    Soft,
    Eased,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub duration: Duration,
    pub band: f32,
    pub tint: [f32; 4],
    pub sweep: Sweep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Duration,
    Band,
    Tint,
}

fn linear_channel(value: f32) -> f32 {
    match value <= 0.04045 {
        true => value / 12.92,
        false => ((value + 0.055) / 1.055).powf(2.4),
    }
}

fn linear_of(srgb: color::Srgba) -> [f32; 4] {
    [
        linear_channel(srgb[0]),
        linear_channel(srgb[1]),
        linear_channel(srgb[2]),
        srgb[3],
    ]
}

impl Settings {
    fn checked(self) -> Result<Self, Invalid> {
        match (
            self.duration.is_zero(),
            self.band.is_finite() && self.band > 0.0 && self.band <= 1.0,
            self.tint.iter().all(|channel| channel.is_finite()),
        ) {
            (true, _, _) => Err(Invalid::Duration),
            (_, false, _) => Err(Invalid::Band),
            (_, _, false) => Err(Invalid::Tint),
            _ => Ok(self),
        }
    }

    pub fn plain(request: SnapshotTransition) -> Result<Self, Invalid> {
        Self {
            duration: request.duration,
            band: request.band,
            tint: request.shimmer.to_linear(),
            sweep: Sweep::Soft,
        }
        .checked()
    }

    pub fn gpui(request: GpuiSnapshotTransition) -> Result<Self, Invalid> {
        Self {
            duration: request.duration,
            band: request.band,
            tint: linear_of(color::hsla(request.shimmer)),
            sweep: Sweep::Eased,
        }
        .checked()
    }

    pub fn gpui_source(request: GpuiSourceSnapshotTransition) -> Result<Self, Invalid> {
        Self {
            duration: request.duration,
            band: request.band,
            tint: linear_of(color::text_color(request.shimmer)),
            sweep: Sweep::Eased,
        }
        .checked()
    }
}

pub fn ease(value: f32) -> f32 {
    let bezier = |s: f32, first: f32, second: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * first + 3.0 * u * s * s * second + s * s * s
    };
    match value {
        value if value <= 0.0 => 0.0,
        value if value >= 1.0 => 1.0,
        value => {
            let (low, high) = (0..24).fold((0.0_f32, 1.0_f32), |(low, high), _| {
                let mid = (low + high) * 0.5;
                match bezier(mid, 0.65, 0.35) < value {
                    true => (mid, high),
                    false => (low, mid),
                }
            });
            bezier((low + high) * 0.5, 0.0, 1.0)
        }
    }
}

pub fn progress(settings: &Settings, elapsed: Duration) -> f32 {
    let linear = (elapsed.as_secs_f64() / settings.duration.as_secs_f64()).clamp(0.0, 1.0) as f32;
    match settings.sweep {
        Sweep::Soft => linear,
        Sweep::Eased => ease(linear),
    }
}

#[derive(Clone, Debug)]
pub struct Transition {
    previous: Vec<PaintCommand>,
    settings: Settings,
    started: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Running(f32),
    Finished,
}

impl Transition {
    pub fn begin(previous: Vec<PaintCommand>, settings: Settings) -> Self {
        Self {
            previous,
            settings,
            started: None,
        }
    }

    pub fn advance(&mut self, now: Instant) -> Phase {
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_duration_since(started);
        match elapsed >= self.settings.duration {
            true => Phase::Finished,
            false => Phase::Running(progress(&self.settings, elapsed)),
        }
    }

    pub fn overlay(&self, surface: Rect, progress: f32, out: &mut Vec<PaintCommand>) {
        let spec = CustomPassSpec {
            name: NAME.into(),
            hlsl: HLSL.into(),
            entry: ENTRY.into(),
            bounds: surface,
            params: parameters(&self.settings, surface, progress),
        };
        out.push(PaintCommand::PushCustomEffect {
            bounds: surface,
            spec,
        });
        out.extend(self.previous.iter().cloned());
        out.push(PaintCommand::PopCustomEffect);
    }
}

pub fn parameters(settings: &Settings, surface: Rect, progress: f32) -> Vec<f32> {
    let mode = match settings.sweep {
        Sweep::Soft => 0.0,
        Sweep::Eased => 2.0,
    };
    vec![
        surface.w,
        surface.h,
        progress,
        settings.band,
        settings.tint[0],
        settings.tint[1],
        settings.tint[2],
        settings.tint[3],
        mode,
        0.0,
        0.0,
        0.0,
    ]
}

fn smoothstep(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn weights(parameters: &[f32], x: f32, y: f32) -> (f32, f32) {
    let (width, height, progress, band) =
        (parameters[0], parameters[1], parameters[2], parameters[3]);
    let tint_alpha = parameters[7];
    let t = (x / width + y / height) * 0.5;
    match parameters[8] < 1.0 {
        true => {
            let edge = -band + progress * (1.0 + 2.0 * band);
            let reveal = 1.0 - smoothstep(edge - band * 0.5, edge + band * 0.5, t);
            (1.0 - reveal, (1.0 - (2.0 * reveal - 1.0).abs()) * tint_alpha)
        }
        false => {
            let edge = -band + progress * (1.0 + 2.0 * band);
            let keep = smoothstep(edge - 0.008, edge + 0.008, t);
            let offset = (t - edge) / band.max(0.0001) + 0.5;
            let lobe = (offset.min(1.0 - offset) / 0.4).clamp(0.0, 1.0);
            (keep, tint_alpha * lobe * lobe * (3.0 - 2.0 * lobe))
        }
    }
}

fn kernel(spec: &CustomPassSpec, inputs: &[&Layer], target: &mut Layer) {
    let Some(source) = inputs.first() else {
        return;
    };
    if source.width != target.width || source.height != target.height || spec.params.len() < 9 {
        return;
    }
    let tint = &spec.params[4..7];
    for y in 0..target.height {
        for x in 0..target.width {
            let old = source.at(x, y);
            let (keep, strength) = weights(&spec.params, x as f32 + 0.5, y as f32 + 0.5);
            let kept = (1.0 - strength) * keep;
            target.put(
                x,
                y,
                [
                    old[0] * kept + tint[0] * strength,
                    old[1] * kept + tint[1] * strength,
                    old[2] * kept + tint[2] * strength,
                    1.0 - (1.0 - strength) * (1.0 - keep),
                ],
            );
        }
    }
}

pub fn register(kernels: &mut Kernels) {
    kernels.register_custom(NAME, Box::new(kernel));
}
