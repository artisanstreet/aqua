use aqua_platform::geometry::LogicalSize;
use aqua_platform::window::{WindowError, WindowOptions};

#[test]
fn native_size_constraints_reject_nonfinite_and_inverted_input() {
    let mut options = WindowOptions::default();
    for size in [
        LogicalSize::new(f64::INFINITY, 20.0),
        LogicalSize::new(20.0, f64::NAN),
        LogicalSize::new(-1.0, 20.0),
    ] {
        options.max_size = Some(size);
        assert_eq!(options.validated(), Err(WindowError::EmptySize));
    }
    options.max_size = Some(LogicalSize::new(100.0, 200.0));
    options.min_size = Some(LogicalSize::new(200.0, 100.0));
    assert_eq!(
        options.validated(),
        Err(WindowError::InvalidSizeConstraints)
    );
    options.max_size = Some(LogicalSize::new(200.0, 200.0));
    assert!(options.validated().is_ok());
    options.titlebar_height = f64::INFINITY;
    assert_eq!(options.validated(), Err(WindowError::InvalidChromeHeight));
}
