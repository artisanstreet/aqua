use aqua_core::Point;
use aqua_shell::FileDrag;
use aqua_shell_appkit::model::drop::{DragInput, DropTarget};
use std::path::PathBuf;

fn run(target: DropTarget, inputs: Vec<DragInput>) -> (DropTarget, Vec<FileDrag>, Vec<bool>) {
    inputs.into_iter().fold(
        (target, Vec::new(), Vec::new()),
        |(target, mut reports, mut copies), input| {
            let step = target.step(input);
            reports.extend(step.report);
            copies.push(step.copy);
            (step.target, reports, copies)
        },
    )
}

fn files() -> Vec<PathBuf> {
    vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b")]
}

#[test]
fn dragging_files_in_and_dropping_reports_enter_over_and_drop_with_the_entered_paths() {
    let (_, reports, copies) = run(
        DropTarget::Disabled.enabled(),
        vec![
            DragInput::Entered {
                paths: files(),
                position: Point::new(10.0, 20.0),
            },
            DragInput::Updated {
                position: Point::new(30.0, 40.0),
            },
            DragInput::Performed {
                position: Point::new(31.0, 41.0),
            },
        ],
    );
    assert_eq!(
        reports,
        vec![
            FileDrag::Enter {
                paths: files(),
                position: Point::new(10.0, 20.0)
            },
            FileDrag::Over {
                position: Point::new(30.0, 40.0)
            },
            FileDrag::Drop {
                paths: files(),
                position: Point::new(31.0, 41.0)
            },
        ]
    );
    assert_eq!(copies, vec![true, true, true]);
}

#[test]
fn a_drag_without_files_is_refused_and_reports_nothing() {
    let (target, reports, copies) = run(
        DropTarget::Disabled.enabled(),
        vec![
            DragInput::Entered {
                paths: Vec::new(),
                position: Point::new(1.0, 1.0),
            },
            DragInput::Updated {
                position: Point::new(2.0, 2.0),
            },
            DragInput::Performed {
                position: Point::new(2.0, 2.0),
            },
        ],
    );
    assert!(reports.is_empty());
    assert_eq!(copies, vec![false, false, false]);
    assert_eq!(target, DropTarget::Disabled.enabled());
}

#[test]
fn a_window_that_never_enabled_drops_refuses_files() {
    let (target, reports, copies) = run(
        DropTarget::Disabled,
        vec![DragInput::Entered {
            paths: files(),
            position: Point::new(1.0, 1.0),
        }],
    );
    assert!(reports.is_empty());
    assert_eq!(copies, vec![false]);
    assert_eq!(target, DropTarget::Disabled);
}

#[test]
fn leaving_ends_the_drag_so_a_later_drop_reports_nothing() {
    let (_, reports, _) = run(
        DropTarget::Disabled.enabled(),
        vec![
            DragInput::Entered {
                paths: files(),
                position: Point::new(1.0, 1.0),
            },
            DragInput::Exited,
            DragInput::Performed {
                position: Point::new(2.0, 2.0),
            },
        ],
    );
    assert_eq!(
        reports,
        vec![
            FileDrag::Enter {
                paths: files(),
                position: Point::new(1.0, 1.0)
            },
            FileDrag::Leave
        ]
    );
}
