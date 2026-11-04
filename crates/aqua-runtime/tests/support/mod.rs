#![allow(dead_code)]

use aqua_protocol::{
    A11yActionRequest, Instant, Machine, PixelSize, RuntimeInput, RuntimeOutput, ShellEvent,
    WindowId,
};
use aqua_runtime::Runtime;

pub fn window() -> WindowId {
    WindowId::new(1).unwrap()
}

pub fn frame(runtime: &mut Runtime, secs: f64) -> RuntimeOutput {
    runtime.step(RuntimeInput::Shell(ShellEvent::Frame {
        window: window(),
        presentation: Instant::from_secs_f64(secs),
    }))
}

pub fn tick(runtime: &mut Runtime, secs: f64) -> RuntimeOutput {
    runtime.step(RuntimeInput::Tick(Instant::from_secs_f64(secs)))
}

pub fn resize(runtime: &mut Runtime, width: f32, height: f32, scale: f32) -> RuntimeOutput {
    runtime.step(RuntimeInput::Shell(ShellEvent::Resized {
        window: window(),
        size: PixelSize {
            width: (width * scale).round() as u32,
            height: (height * scale).round() as u32,
        },
        scale,
    }))
}

pub fn access(runtime: &mut Runtime, request: A11yActionRequest) -> RuntimeOutput {
    runtime.step(RuntimeInput::Access(request))
}
