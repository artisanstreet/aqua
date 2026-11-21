use objc2_app_kit::NSScreen;
use objc2_foundation::{MainThreadMarker, NSRect};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub bounds: (i32, i32, i32, i32),
    pub work_area: (i32, i32, i32, i32),
    pub scale: f64,
    pub refresh: Option<Duration>,
}

pub(crate) fn primary_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |primary| primary.frame().size.height)
}

fn physical(rect: NSRect, scale: f64, primary_height: f64) -> (i32, i32, i32, i32) {
    let left = rect.origin.x * scale;
    let top = (primary_height - rect.origin.y - rect.size.height) * scale;
    (
        left.round() as i32,
        top.round() as i32,
        (left + rect.size.width * scale).round() as i32,
        (top + rect.size.height * scale).round() as i32,
    )
}

pub(crate) fn refresh_of(screen: &NSScreen) -> Option<Duration> {
    let interval = screen.minimumRefreshInterval();
    (interval > 0.0 && interval.is_finite()).then(|| Duration::from_secs_f64(interval))
}

pub(crate) fn monitor_of(mtm: MainThreadMarker, screen: &NSScreen) -> Monitor {
    let scale = screen.backingScaleFactor();
    let primary = primary_height(mtm);
    Monitor {
        name: screen.localizedName().to_string(),
        bounds: physical(screen.frame(), scale, primary),
        work_area: physical(screen.visibleFrame(), scale, primary),
        scale,
        refresh: refresh_of(screen),
    }
}
