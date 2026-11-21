use super::{available, window_number, Facility};
use crate::window::Window;
use image::RgbaImage;
use std::process::Command;

pub fn window(window: Window) -> Option<RgbaImage> {
    if !available(Facility::ScreenCapture) {
        return None;
    }
    let number = window_number(window)?;
    let path = std::env::temp_dir().join(format!(
        "aqua-appkit-capture-{}-{number}.png",
        std::process::id()
    ));
    let status = Command::new("screencapture")
        .arg("-x")
        .arg("-o")
        .arg(format!("-l{number}"))
        .arg(&path)
        .status()
        .expect("screencapture runs on macOS");
    assert!(
        status.success(),
        "screencapture -l{number} failed with {status}"
    );
    let image = image::open(&path)
        .expect("screencapture writes a PNG")
        .to_rgba8();
    std::fs::remove_file(&path).expect("the capture file can be removed");
    Some(image)
}
