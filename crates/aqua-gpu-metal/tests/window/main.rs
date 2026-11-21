#[cfg(target_os = "macos")]
mod composition;
#[cfg(target_os = "macos")]
mod custom;
#[cfg(target_os = "macos")]
mod pacing;
#[cfg(target_os = "macos")]
mod present;
#[cfg(target_os = "macos")]
mod recovery;
#[cfg(target_os = "macos")]
mod resize;
#[cfg(target_os = "macos")]
mod scale;
#[cfg(target_os = "macos")]
mod support;
#[cfg(target_os = "macos")]
mod teardown;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("window: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{case, Case};

    pub const ALL: [Case; 10] = [
        case(
            "the_presented_drawable_equals_the_offscreen_render",
            super::present::drawable_equals_offscreen,
        ),
        case(
            "the_screen_shows_what_was_presented",
            super::present::screen_shows_presented,
        ),
        case(
            "a_runtime_custom_pass_shows_on_screen",
            super::custom::custom_pass_shows_on_screen,
        ),
        case(
            "a_premultiplied_window_composites_over_the_window_behind",
            super::composition::premultiplied_over_window_behind,
        ),
        case(
            "a_resize_storm_never_presents_a_stretched_or_stale_size",
            super::resize::resize_storm,
        ),
        case(
            "presentation_feedback_reports_the_display_interval",
            super::pacing::display_interval,
        ),
        case(
            "a_hidden_window_costs_one_bounded_wait_and_then_nothing_beside_a_visible_one",
            super::pacing::hidden_window_bounded,
        ),
        case(
            "device_loss_with_two_live_windows_recovers_both",
            super::recovery::device_loss_recovers_both,
        ),
        case(
            "closing_with_frames_in_flight_does_not_crash",
            super::teardown::closing_with_frames_in_flight,
        ),
        case(
            "a_backing_scale_change_resizes_the_drawable_on_the_next_frame",
            super::scale::backing_scale_change,
        ),
    ];
}
