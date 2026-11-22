use aqua_protocol::{BadgeError, TaskbarBadge};
use objc2_foundation::{MainThreadMarker, NSString};

fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("invariant: the dock tile is changed from the main thread")
}

pub fn set_badge(badge: Option<TaskbarBadge>) -> Result<(), BadgeError> {
    let mtm = main_thread();
    let label = badge
        .filter(|badge| badge.count > 0)
        .map(|badge| NSString::from_str(&badge.label()));
    crate::app::shared(mtm)
        .dockTile()
        .setBadgeLabel(label.as_deref());
    Ok(())
}

pub fn badge_label() -> Option<String> {
    crate::app::shared(main_thread())
        .dockTile()
        .badgeLabel()
        .map(|label| label.to_string())
}
