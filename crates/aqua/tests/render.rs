#[path = "render/animation.rs"]
mod animation;
#[path = "render/cadence.rs"]
mod cadence;
#[path = "render/color.rs"]
mod color;
#[path = "render/cull.rs"]
mod cull;
#[path = "render/host.rs"]
mod host;
#[path = "render/lowering.rs"]
mod lowering;
#[path = "render/mesh.rs"]
mod mesh;
#[path = "render/shape.rs"]
mod shape;
#[path = "render/span.rs"]
mod span;
mod support;
#[path = "render/text.rs"]
mod text;
#[cfg(windows)]
#[path = "render/lower/dpi.rs"]
mod text_dpi;
#[path = "render/vector.rs"]
mod vector;
