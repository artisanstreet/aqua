use super::{available, window_number, Facility};
use crate::window::Window;
use image::RgbaImage;
use std::process::Command;

fn screencapture(label: &str, args: &[String]) -> RgbaImage {
    let path = std::env::temp_dir().join(format!(
        "aqua-appkit-capture-{}-{label}.png",
        std::process::id()
    ));
    let status = Command::new("screencapture")
        .arg("-x")
        .args(args)
        .arg(&path)
        .status()
        .expect("screencapture runs on macOS");
    assert!(
        status.success(),
        "screencapture {args:?} failed with {status}"
    );
    let image = image::open(&path)
        .expect("screencapture writes a PNG")
        .to_rgba8();
    std::fs::remove_file(&path).expect("the capture file can be removed");
    image
}

pub fn window(window: Window) -> Option<RgbaImage> {
    if !available(Facility::ScreenCapture) {
        return None;
    }
    let number = window_number(window)?;
    Some(window_number_image(number))
}

pub fn window_number_image(number: isize) -> RgbaImage {
    screencapture(
        &format!("window-{number}"),
        &["-o".to_owned(), format!("-l{number}")],
    )
}

pub fn screen(x: f64, y: f64, width: f64, height: f64) -> Option<RgbaImage> {
    if !available(Facility::ScreenCapture) {
        return None;
    }
    Some(screen_image(x, y, width, height))
}

pub fn screen_image(x: f64, y: f64, width: f64, height: f64) -> RgbaImage {
    screencapture("screen", &[format!("-R{x},{y},{width},{height}")])
}
