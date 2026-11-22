use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_gpu_metal::window::Alpha;
use aqua_graph::{CustomPassSpec, Extent, FramePlan, PaintCommand};
use aqua_protocol::{GpuBackend, PresentHint};
use aqua_shell_appkit::testing::capture;

use super::support::{
    self, drive, extent_of, frame, mismatches, painted, shown, srgb, Flow, RenderThread, CAPTURES,
    LIMIT,
};

const RAMP_HLSL: &str = "struct CustomInput { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
float4 ramp(CustomInput input) : SV_Target { return float4(input.uv.x, 0.45, 1.0 - input.uv.x, 1.0); }\n";

const INSET: f32 = 16.0;

fn ramp(extent: Extent) -> FramePlan {
    let bounds = Rect {
        x: INSET,
        y: INSET,
        w: extent.width as f32 - 2.0 * INSET,
        h: extent.height as f32 - 2.0 * INSET,
    };
    let spec = CustomPassSpec {
        isolate_backdrop: false,
        reference: None,
        name: "window-ramp".into(),
        hlsl: RAMP_HLSL.into(),
        entry: "ramp".into(),
        bounds,
        params: Vec::new(),
    };
    painted(
        extent,
        srgb(240, 240, 236),
        &[
            PaintCommand::PushCustomEffect { bounds, spec },
            PaintCommand::PopCustomEffect,
        ],
    )
}

fn texel(rgba: &[u8], extent: Extent, x: u32, y: u32) -> [u8; 3] {
    let offset = ((y * extent.width + x) * 4) as usize;
    [rgba[offset], rgba[offset + 1], rgba[offset + 2]]
}

pub fn custom_pass_shows_on_screen() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    if !support::compiling() {
        return;
    }
    assert!(
        gpu.run(|backend| backend.capabilities().features.custom_passes),
        "the window backend compiles custom passes where the offscreen one does"
    );
    let window = support::window("metal custom", (240.0, 160.0), (40, 80));
    let extent = extent_of(window);
    let surface = gpu.attach(window, Alpha::Opaque);
    let plan = ramp(extent);
    let expected = support::offscreen(&plan);
    let middle = extent.height / 2;
    let inset = INSET as u32 + 2;
    let left = texel(&expected, extent, inset, middle);
    let right = texel(&expected, extent, extent.width - inset, middle);
    assert!(
        left[0] < 96 && left[2] > 160 && right[0] > 160 && right[2] < 96,
        "the custom pass paints its ramp, from {left:?} to {right:?}"
    );
    let mut captures = 0;
    drive(LIMIT, move |_, _| {
        let drawn = plan.clone();
        gpu.run(move |backend| frame(backend, surface, &drawn, PresentHint::Vsync));
        if shown(&gpu, surface) < 3 {
            return Flow::Wait(Duration::from_millis(10));
        }
        let Some(image) = capture::window(window) else {
            return Flow::Done;
        };
        captures += 1;
        let last = mismatches(&image, &expected, extent, 3);
        match last.is_empty() {
            true => {
                gpu.assert_clean();
                Flow::Done
            }
            false if captures < CAPTURES => Flow::Wait(Duration::from_millis(30)),
            false => panic!(
                "{} of {} on-screen pixels differ from the offscreen custom pass after {captures} captures, first at {:?}",
                last.len(),
                extent.texels(),
                last.first()
            ),
        }
    });
}
