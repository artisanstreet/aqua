mod raster;
mod svg;

pub use raster::{
    RasterizedVector, VectorBudget, VectorError, curve_outline, rasterize, retained_source_bytes,
};

pub use svg::{CurveOp, CurveScene, drawing_curves, svg_curves};
