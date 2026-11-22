#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    match std::env::var(cases::SOURCE) {
        Ok(paths) => cases::source(&paths),
        Err(_) => support::main(&cases::ALL),
    }
    #[cfg(not(target_os = "macos"))]
    println!("drop: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, script, stage, Case, Context, Flow, Stage, POLL};
    use aqua_core::Point;
    use aqua_shell_appkit::testing::drag::{self, FakeDrag};
    use aqua_shell_appkit::testing::inject::{Button, Motion};
    use aqua_shell_appkit::testing::{self, post};
    use aqua_shell_appkit::{FileDrag, ShellEvent, Window};
    use objc2_app_kit::NSDragOperation;
    use std::cell::RefCell;
    use std::io::{BufRead, BufReader};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::rc::Rc;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Duration;

    pub const SOURCE: &str = "AQUA_DROP_SOURCE";

    pub const ALL: [Case; 3] = [
        case(
            "dragging_files_from_another_process_reports_enter_over_and_drop_where_released",
            from_another_process,
        ),
        case(
            "drag_positions_are_view_local_points_from_the_top_left",
            positions,
        ),
        case("a_window_without_file_drop_refuses_the_drag", refused),
    ];

    const LIMIT: Duration = Duration::from_secs(30);
    const SIZE: (f64, f64) = (320.0, 240.0);

    fn files() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/tmp/aqua drop/one.txt"),
            PathBuf::from("/tmp/aqua drop/two"),
        ]
    }

    fn create_files() {
        std::fs::create_dir_all("/tmp/aqua drop/two").expect("the dragged folder exists");
        std::fs::write("/tmp/aqua drop/one.txt", "one").expect("the dragged file exists");
    }

    fn target(title: &str, enable: bool) -> Window {
        let window = Window::create(&support::options(title, SIZE)).expect("window");
        window.set_position(80, 120);
        if enable {
            window.enable_file_drop();
        }
        window
    }

    fn drags(cx: &Context<'_>) -> Vec<FileDrag> {
        cx.log
            .iter()
            .filter_map(|(_, event, _)| match event {
                ShellEvent::FileDrag(drag) => Some(drag.clone()),
                _ => None,
            })
            .collect()
    }

    fn all_drags(log: &[support::Entry]) -> Vec<FileDrag> {
        log.iter()
            .filter_map(|(_, event, _)| match event {
                ShellEvent::FileDrag(drag) => Some(drag.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn source(paths: &str) {
        let paths: Vec<PathBuf> = paths.lines().map(PathBuf::from).collect();
        let source = drag::source_window((520.0, 120.0), (160.0, 160.0), &paths);
        println!("ready {} {}", source.centre.0, source.centre.1);
        support::drive(LIMIT, |_, _| match drag::source_ended() {
            true => Flow::Done,
            false => Flow::Wait(POLL),
        });
        println!("ended");
    }

    struct Peer {
        child: Child,
        lines: Receiver<String>,
        seen: Rc<RefCell<Vec<String>>>,
    }

    impl Peer {
        fn pump(&self) -> Vec<String> {
            let mut seen = self.seen.borrow_mut();
            seen.extend(self.lines.try_iter());
            seen.clone()
        }

        fn wait_for(&self, prefix: &str, limit: Duration) -> String {
            let deadline = std::time::Instant::now() + limit;
            loop {
                if let Some(line) = self
                    .pump()
                    .into_iter()
                    .find(|line| line.starts_with(prefix))
                {
                    return line;
                }
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                match self.lines.recv_timeout(left) {
                    Ok(line) => self.seen.borrow_mut().push(line),
                    Err(_) => panic!("the drag source never said {prefix:?}"),
                }
            }
        }

        fn said(&self, line: &str) -> bool {
            self.pump().iter().any(|said| said == line)
        }
    }

    impl Drop for Peer {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            println!("drag source said: {:?}", self.pump());
        }
    }

    fn start_source() -> Rc<Peer> {
        let exe = std::env::current_exe().expect("the test binary knows its path");
        let joined = files()
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let mut child = Command::new(exe)
            .env(SOURCE, joined)
            .stdout(Stdio::piped())
            .spawn()
            .expect("the drag source starts");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Rc::new(Peer {
            child,
            lines,
            seen: Rc::new(RefCell::new(Vec::new())),
        })
    }

    fn drag_step(point: (f64, f64)) -> Stage {
        stage(
            "drag step",
            move || post::mouse_at(Button::Left, Motion::Dragged, point, 1),
            |cx| cx.elapsed() >= POLL,
        )
    }

    fn path(from: (f64, f64), to: (f64, f64), steps: usize) -> Vec<(f64, f64)> {
        (1..=steps)
            .map(|step| {
                let t = step as f64 / steps as f64;
                (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
            })
            .collect()
    }

    fn activate() -> Stage {
        stage("activate", testing::activate, |cx| {
            cx.saw(|event| *event == ShellEvent::Focused(true))
        })
    }

    fn from_another_process() {
        if !support::posting() {
            return;
        }
        create_files();
        let window = target("drop target", true);
        let peer = start_source();
        let ready = peer.wait_for("ready ", Duration::from_secs(20));
        let mut words = ready
            .split_whitespace()
            .skip(1)
            .map(|word| word.parse::<f64>().expect("the drag source prints numbers"));
        let grab = (words.next().expect("x"), words.next().expect("y"));
        let entry = testing::screen_point(window, (40.0, 40.0)).expect("the target is on screen");
        let release_local = (200.0, 150.0);
        let release =
            testing::screen_point(window, release_local).expect("the target is on screen");
        let pressed = peer.clone();
        let mut stages = vec![
            activate(),
            stage(
                "press on the source",
                move || {
                    post::mouse_at(Button::Left, Motion::Moved, grab, 0);
                    post::mouse_at(Button::Left, Motion::Down, grab, 1);
                },
                move |_| pressed.said("began"),
            ),
        ];
        stages.extend(path(grab, entry, 16).into_iter().map(drag_step));
        stages.extend(path(entry, release, 8).into_iter().map(drag_step));
        stages.extend(
            (0..20)
                .map(|step| {
                    let offset = if step % 2 == 0 { 2.0 } else { -2.0 };
                    (release.0 + offset, release.1)
                })
                .map(drag_step),
        );
        stages.push(drag_step(release));
        stages.push(stage(
            "release",
            move || post::mouse_at(Button::Left, Motion::Up, release, 1),
            |cx| {
                cx.elapsed() >= Duration::from_secs(5)
                    || drags(cx)
                        .iter()
                        .any(|drag| matches!(drag, FileDrag::Drop { .. }))
            },
        ));
        let log = script(LIMIT, stages);
        let drags = all_drags(&log);
        let Some(FileDrag::Enter { paths, .. }) = drags.first() else {
            panic!(
                "the drag starts with Enter: {drags:#?}; the window's drop state is {}",
                drag::drop_state(window)
            );
        };
        assert_eq!(paths, &files());
        assert!(
            drags
                .iter()
                .any(|drag| matches!(drag, FileDrag::Over { .. })),
            "moving inside reports Over: {drags:#?}"
        );
        let Some(FileDrag::Drop { paths, position }) = drags.last() else {
            panic!("the drag ends with Drop: {drags:#?}");
        };
        assert_eq!(paths, &files());
        assert!(
            (position.x - release_local.0 as f32).abs() <= 1.0
                && (position.y - release_local.1 as f32).abs() <= 1.0,
            "the drop lands where the mouse was released: {position:?}"
        );
    }

    fn positions() {
        if !support::appkit() {
            return;
        }
        let window = target("drop positions", true);
        let fake = FakeDrag::carrying(window, &files());
        let operations = Rc::new(RefCell::new(Vec::new()));
        let seen = operations.clone();
        let log = script(
            LIMIT,
            vec![stage(
                "drag over and drop",
                move || {
                    seen.borrow_mut().push(fake.enter((12.0, 8.0)));
                    seen.borrow_mut().push(fake.update((150.5, 200.0)));
                    assert!(fake.perform((300.0, 230.0)), "the drop is accepted");
                },
                |cx| drags(cx).len() == 3,
            )],
        );
        assert_eq!(
            all_drags(&log),
            vec![
                FileDrag::Enter {
                    paths: files(),
                    position: Point::new(12.0, 8.0)
                },
                FileDrag::Over {
                    position: Point::new(150.5, 200.0)
                },
                FileDrag::Drop {
                    paths: files(),
                    position: Point::new(300.0, 230.0)
                },
            ]
        );
        assert_eq!(*operations.borrow(), vec![NSDragOperation::Copy; 2]);
    }

    fn refused() {
        if !support::appkit() {
            return;
        }
        let window = target("no drops", false);
        let fake = FakeDrag::carrying(window, &files());
        let operations = Rc::new(RefCell::new(Vec::new()));
        let seen = operations.clone();
        let log = script(
            LIMIT,
            vec![stage(
                "drag over and drop",
                move || {
                    seen.borrow_mut().push(fake.enter((12.0, 8.0)));
                    seen.borrow_mut().push(fake.update((40.0, 40.0)));
                    let accepted = fake.perform((40.0, 40.0));
                    assert!(!accepted, "a refused drag cannot drop");
                    fake.exit();
                },
                |cx| cx.elapsed() >= POLL,
            )],
        );
        assert!(all_drags(&log).is_empty(), "nothing reaches the app");
        assert_eq!(*operations.borrow(), vec![NSDragOperation::None; 2]);
    }
}
