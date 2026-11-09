mod raster;
mod svg;

pub use raster::{
    curve_outline, rasterize, retained_source_bytes, RasterizedVector, VectorBudget, VectorError,
};

pub use svg::{drawing_curves, svg_curves, CurveOp, CurveScene};
