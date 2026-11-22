#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    support::main(&cases::ALL);
    #[cfg(not(target_os = "macos"))]
    println!("cursor: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, script, stage, Case};
    use aqua_core::CursorIcon;
    use aqua_shell_appkit::testing::inject::{Button, Motion};
    use aqua_shell_appkit::testing::{self, post};
    use aqua_shell_appkit::{ShellEvent, Window};
    use std::time::Duration;

    pub const ALL: [Case; 1] = [case("the_ibeam_cursor_is_set_over_text", ibeam)];

    fn ibeam() {
        if !support::posting() {
            return;
        }
        let window = Window::create(&support::options("cursor", (320.0, 240.0))).expect("window");
        window.set_cursor(CursorIcon::Text);
        script(
            Duration::from_secs(20),
            vec![
                stage("activate", testing::activate, |cx| {
                    cx.saw(|event| *event == ShellEvent::Focused(true))
                }),
                stage(
                    "outside",
                    move || post::mouse(window, Button::Left, Motion::Moved, (-80.0, 100.0), 0),
                    |cx| cx.elapsed() >= Duration::from_millis(100),
                ),
                stage(
                    "over text",
                    move || post::mouse(window, Button::Left, Motion::Moved, (100.0, 100.0), 0),
                    |cx| cx.saw(|event| *event == ShellEvent::PointerEntered),
                ),
                stage(
                    "ibeam",
                    move || post::mouse(window, Button::Left, Motion::Moved, (110.0, 104.0), 0),
                    |_| testing::current_cursor() == Some(CursorIcon::Text),
                ),
                stage(
                    "link",
                    move || window.set_cursor(CursorIcon::Pointer),
                    |_| testing::current_cursor() == Some(CursorIcon::Pointer),
                ),
                stage(
                    "leave",
                    move || post::mouse(window, Button::Left, Motion::Moved, (400.0, 100.0), 0),
                    |cx| {
                        cx.saw(|event| *event == ShellEvent::PointerLeft)
                            && testing::current_cursor() == Some(CursorIcon::Default)
                    },
                ),
            ],
        );
    }
}
