#![cfg(target_os = "linux")]

mod support;

use aqua_shell_wayland::{FrameWait, Presented, ShellEvent, SurfaceLease, Window};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;
use support::{drive, options, Flow};

enum Ask {
    Prepare((u32, u32)),
    Wait(Duration),
    Presented,
    Stop,
}

enum Answer {
    Prepared,
    Waited(FrameWait, Duration),
    Presented(Vec<Presented>),
}

fn render_thread(
    lease: SurfaceLease,
) -> (Sender<Ask>, Receiver<Answer>, std::thread::JoinHandle<()>) {
    let (ask, asks) = channel::<Ask>();
    let (answer, answers) = channel::<Answer>();
    let thread = std::thread::spawn(move || {
        while let Ok(request) = asks.recv() {
            let reply = match request {
                Ask::Prepare(buffer) => {
                    lease.before_present(buffer);
                    Answer::Prepared
                }
                Ask::Wait(timeout) => {
                    let started = std::time::Instant::now();
                    let outcome = lease
                        .wait_ready(timeout)
                        .expect("the lease queue dispatches");
                    Answer::Waited(outcome, started.elapsed())
                }
                Ask::Presented => Answer::Presented(lease.take_presented()),
                Ask::Stop => break,
            };
            if answer.send(reply).is_err() {
                break;
            }
        }
    });
    (ask, answers, thread)
}

fn present_once(
    window: Window,
    ask: &Sender<Ask>,
    answers: &Receiver<Answer>,
    timeout: Duration,
) -> (FrameWait, Duration) {
    ask.send(Ask::Prepare(window.inner_size())).unwrap();
    assert!(matches!(answers.recv().unwrap(), Answer::Prepared));
    support::paint(window);
    ask.send(Ask::Wait(timeout)).unwrap();
    match answers.recv().unwrap() {
        Answer::Waited(outcome, waited) => (outcome, waited),
        _ => unreachable!("the render thread answers in order"),
    }
}

#[derive(Default)]
struct Results {
    visible: Vec<(FrameWait, Duration)>,
    hidden: Vec<(FrameWait, Duration)>,
    presented: Vec<Presented>,
}

#[test]
fn a_render_thread_lease_paces_on_frame_callbacks_and_never_blocks_on_a_hidden_window() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let app = "com.aqua.test.lease";
    let window = Window::create(&options(app, (320.0, 240.0))).unwrap();
    let lease = window
        .surface_lease()
        .expect("a live window hands out a lease");
    assert_eq!(lease.window(), window.id());
    assert_ne!(lease.display_handle(), 0);
    assert_ne!(lease.surface_handle(), 0);
    let (ask, answers, thread) = render_thread(lease);
    let results = Rc::new(RefCell::new(Results::default()));
    let recorded = results.clone();
    let mut phase = 0;
    let probe = compositor.clone();
    drive(&[window], Duration::from_secs(20), move |context| {
        let mut results = recorded.borrow_mut();
        match phase {
            0 if support::ready(&probe, app).is_some() && context.focused() => phase = 1,
            1 => {
                results.visible.push(present_once(
                    window,
                    &ask,
                    &answers,
                    Duration::from_millis(500),
                ));
                if results.visible.len() == 10 {
                    ask.send(Ask::Presented).unwrap();
                    if let Answer::Presented(frames) = answers.recv().unwrap() {
                        results.presented = frames;
                    }
                    probe
                        .command(&format!("[app_id=\"{app}\"] move container to workspace 2"))
                        .unwrap();
                    phase = 2;
                }
            }
            2 => {
                results.hidden.push(present_once(
                    window,
                    &ask,
                    &answers,
                    Duration::from_millis(50),
                ));
                if results.hidden.len() == 10 {
                    ask.send(Ask::Stop).unwrap();
                    return Flow::Done;
                }
            }
            _ => {}
        }
        Flow::Wait(Duration::from_millis(5))
    });
    thread.join().unwrap();
    let results = results.borrow();
    assert!(
        results
            .visible
            .iter()
            .all(|(outcome, _)| *outcome == FrameWait::Ready),
        "visible frames: {:?}",
        results.visible
    );
    assert!(
        results.hidden[2..]
            .iter()
            .all(|(outcome, waited)| *outcome == FrameWait::TimedOut
                && *waited < Duration::from_millis(50 + 20)),
        "a hidden window's waits stay bounded: {:?}",
        results.hidden
    );
    let shown: Vec<_> = results
        .presented
        .iter()
        .filter_map(|frame| match frame {
            Presented::Shown { at, refresh, .. } => Some((*at, *refresh)),
            Presented::Discarded => None,
        })
        .collect();
    assert!(
        !shown.is_empty(),
        "presentation feedback arrived: {:?}",
        results.presented
    );
    assert!(shown.iter().all(|(at, _)| at.is_some()), "{shown:?}");
    let times: Vec<aqua_protocol::Instant> = shown.iter().filter_map(|(at, _)| *at).collect();
    let span = times[times.len() - 1].saturating_duration_since(times[0]);
    let mean = span / u32::try_from(times.len() - 1).unwrap();
    assert!(
        mean.abs_diff(Duration::from_micros(16_667)) < Duration::from_micros(1_667),
        "presentation times follow the 60 Hz output: mean {mean:?} over {shown:?}"
    );
}

#[test]
fn a_lease_outlives_its_window_without_a_protocol_error() {
    let Some(_compositor) = support::compositor() else {
        return;
    };
    let first = Window::create(&options("com.aqua.test.outlive", (200.0, 200.0))).unwrap();
    let second = Window::create(&options("com.aqua.test.survivor", (200.0, 200.0))).unwrap();
    let mut lease = first.surface_lease();
    let mut step = 0;
    let log = drive(&[first, second], Duration::from_secs(5), move |_| {
        step += 1;
        match step {
            1 => first.destroy(),
            2 => {
                let held = lease.as_ref().unwrap();
                held.before_present((200, 200));
                assert_eq!(
                    held.wait_ready(Duration::from_millis(30)).unwrap(),
                    FrameWait::TimedOut
                );
            }
            3 => lease = None,
            6 => return Flow::Done,
            _ => {}
        }
        Flow::Wait(Duration::from_millis(50))
    });
    assert!(support::events(&log).contains(&ShellEvent::Destroyed));
    assert!(second.is_visible());
}
