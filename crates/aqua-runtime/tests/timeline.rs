use std::time::Duration;

use aqua_motion::Easing;
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::Box as UiBox;
use aqua_ui::style::timeline::{Key, Timeline};
use aqua_ui::style::{px, Style};

fn fade() -> Timeline {
    Timeline::new(Duration::from_secs(1)).opacity([
        Key::new(0.0, 0.0, Easing::Linear),
        Key::new(1.0, 1.0, Easing::Linear),
    ])
}

fn mounted(timeline: Timeline) -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(200.0, 100.0, 1.0);
    runtime.advance_clock(10.0);
    runtime
        .mount(move |_| {
            UiBox::new()
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .width(px(50.0))
                                .height(px(20.0))
                                .timeline(timeline),
                        )
                        .build()
                        .with_key("faded"),
                )
                .build()
        })
        .unwrap();
    runtime
}

fn opacity(runtime: &Runtime) -> Option<f32> {
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("faded"));
    let node = runtime.tree().sole_node_with_key(key).unwrap();
    runtime.tree().paint(node).unwrap().opacity
}

#[test]
fn a_timeline_plays_from_mount_without_the_app_rebuilding() {
    let mut runtime = mounted(fade());
    assert_eq!(runtime.build_frame().unwrap(), Some(10.0));
    assert_eq!(opacity(&runtime), Some(0.0));
    runtime.advance_clock(10.5);
    assert_eq!(runtime.build_frame().unwrap(), Some(10.5));
    assert_eq!(opacity(&runtime), Some(0.5));
    runtime.advance_clock(11.0);
    assert_eq!(runtime.build_frame().unwrap(), None);
    assert_eq!(opacity(&runtime), None);
}

#[test]
fn a_node_mounted_mid_run_picks_up_where_it_left_off() {
    let mut runtime = mounted(fade().elapsed(Duration::from_millis(250)));
    runtime.build_frame().unwrap();
    assert_eq!(opacity(&runtime), Some(0.25));
}

#[test]
fn rebuilding_with_the_same_timeline_keeps_the_run_going() {
    let mut runtime = mounted(fade());
    runtime.build_frame().unwrap();
    runtime.advance_clock(10.5);
    runtime
        .refresh_subtree(
            "faded",
            UiBox::new()
                .style(
                    Style::new()
                        .width(px(50.0))
                        .height(px(20.0))
                        .timeline(fade()),
                )
                .build()
                .with_key("faded"),
        )
        .unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(opacity(&runtime), Some(0.5));
}

#[test]
fn reduced_motion_shows_the_resting_paint_at_once() {
    let mut runtime = mounted(fade());
    runtime
        .host_mut()
        .set_reduced_motion(aqua_ui::motion::ReducedMotion::Reduce, 10.0);
    assert_eq!(runtime.build_frame().unwrap(), None);
    assert_eq!(opacity(&runtime), None);
}
