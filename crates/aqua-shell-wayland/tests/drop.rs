#![cfg(target_os = "linux")]

mod support;

use aqua_core::Point;
use aqua_shell_wayland::testing::{DragReport, DragSource, VirtualPointer};
use aqua_shell_wayland::{FileDrag, ShellEvent, Window};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use support::{drive, mapped, options, Entry, Flow};

const TICK: Duration = Duration::from_millis(10);
const SOURCE: &str = "com.aqua.test.drag.source";

fn drags(log: &[Entry]) -> Vec<FileDrag> {
    log.iter()
        .filter_map(|(_, event, _)| match event {
            ShellEvent::FileDrag(drag) => Some(drag.clone()),
            _ => None,
        })
        .collect()
}

fn dragged_files() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/tmp/aqua drop/første bilde.png"),
        PathBuf::from("/tmp/notes.md"),
    ]
}

fn drag_onto(app: &'static str, accept_files: bool) -> Option<(Vec<FileDrag>, DragReport)> {
    let compositor = support::compositor()?;
    let pointer = VirtualPointer::new(&compositor).unwrap();
    let window = Window::create(&options(app, (400.0, 300.0))).unwrap();
    if accept_files {
        window.enable_file_drop();
    }
    let source = Rc::new(DragSource::start(&compositor, SOURCE, &dragged_files()).unwrap());
    let report = source.clone();
    let mut phase = 0;
    let log = drive(&[window], Duration::from_secs(15), move |context| {
        let (Some(target), Some(origin)) = (mapped(&compositor, app), mapped(&compositor, SOURCE))
        else {
            return Flow::Wait(TICK);
        };
        let at = |dx: i32, dy: i32| ((target.x + dx) as u32, (target.y + dy) as u32);
        let state = source.report();
        let entered =
            context.saw(|event| matches!(event, ShellEvent::FileDrag(FileDrag::Enter { .. })));
        let over = context.saw(|event| {
            *event
                == ShellEvent::FileDrag(FileDrag::Over {
                    position: Point::new(90.0, 70.0),
                })
        });
        let settled = state.finished || state.cancelled;
        let laid_out = state.size == (origin.width, origin.height)
            && window.logical_size() == (target.width as u32, target.height as u32);
        phase = match phase {
            0 if laid_out => {
                let centre = (origin.x + origin.width / 2, origin.y + origin.height / 2);
                pointer.move_to(centre.0 as u32, centre.1 as u32).unwrap();
                1
            }
            1 if state.hovered => {
                pointer.press(VirtualPointer::LEFT).unwrap();
                2
            }
            2 if state.started => {
                let (x, y) = at(60, 40);
                pointer.move_to(x, y).unwrap();
                3
            }
            3 if entered || !accept_files => {
                let (x, y) = at(90, 70);
                pointer.move_to(x, y).unwrap();
                4
            }
            4 if over || !accept_files => {
                pointer.release(VirtualPointer::LEFT).unwrap();
                5
            }
            5 if settled => return Flow::Done,
            phase => phase,
        };
        Flow::Wait(TICK)
    });
    Some((drags(&log), report.report()))
}

#[test]
fn a_file_dragged_from_another_client_enters_with_paths_and_drops_at_the_pointer() {
    let Some((drags, report)) = drag_onto("com.aqua.test.drop", true) else {
        return;
    };
    assert_eq!(
        drags.first(),
        Some(&FileDrag::Enter {
            paths: dragged_files(),
            position: Point::new(60.0, 40.0),
        }),
        "{drags:#?}"
    );
    assert!(drags.contains(&FileDrag::Over {
        position: Point::new(90.0, 70.0)
    }));
    assert_eq!(
        drags.last(),
        Some(&FileDrag::Drop {
            paths: dragged_files(),
            position: Point::new(90.0, 70.0),
        }),
        "{drags:#?}"
    );
    assert_eq!(report.accepted.as_deref(), Some("text/uri-list"));
    assert!(report.dropped && report.finished, "{report:?}");
}

#[test]
fn a_window_without_file_drop_refuses_the_drag() {
    let Some((drags, report)) = drag_onto("com.aqua.test.nodrop", false) else {
        return;
    };
    assert_eq!(drags, Vec::new());
    assert!(!report.finished, "{report:?}");
    assert!(report.cancelled, "{report:?}");
}
