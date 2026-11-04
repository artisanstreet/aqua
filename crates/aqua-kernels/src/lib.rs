#![deny(unsafe_op_in_unsafe_fn)]

pub mod color_matrix;
pub mod diff;
pub mod over;

pub type Pixel = [f32; 4];
