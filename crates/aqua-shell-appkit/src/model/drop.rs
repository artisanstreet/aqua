use aqua_core::Point;
use aqua_shell::FileDrag;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DropTarget {
    #[default]
    Disabled,
    Enabled(DragSession),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DragSession {
    #[default]
    Idle,
    Refused,
    Carrying {
        paths: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DragInput {
    Entered {
        paths: Vec<PathBuf>,
        position: Point,
    },
    Updated {
        position: Point,
    },
    Exited,
    Performed {
        position: Point,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DragStep {
    pub target: DropTarget,
    pub report: Option<FileDrag>,
    pub copy: bool,
}

fn step(session: DragSession, input: DragInput) -> (DragSession, Option<FileDrag>, bool) {
    match (session, input) {
        (_, DragInput::Entered { paths, .. }) if paths.is_empty() => {
            (DragSession::Refused, None, false)
        }
        (_, DragInput::Entered { paths, position }) => (
            DragSession::Carrying {
                paths: paths.clone(),
            },
            Some(FileDrag::Enter { paths, position }),
            true,
        ),
        (DragSession::Carrying { paths }, DragInput::Updated { position }) => (
            DragSession::Carrying { paths },
            Some(FileDrag::Over { position }),
            true,
        ),
        (DragSession::Carrying { .. }, DragInput::Exited) => {
            (DragSession::Idle, Some(FileDrag::Leave), false)
        }
        (DragSession::Carrying { paths }, DragInput::Performed { position }) => (
            DragSession::Idle,
            Some(FileDrag::Drop { paths, position }),
            true,
        ),
        (DragSession::Idle | DragSession::Refused, DragInput::Updated { .. }) => {
            (DragSession::Refused, None, false)
        }
        (_, DragInput::Exited | DragInput::Performed { .. }) => (DragSession::Idle, None, false),
    }
}

impl DropTarget {
    pub fn step(self, input: DragInput) -> DragStep {
        match self {
            DropTarget::Disabled => DragStep {
                target: DropTarget::Disabled,
                report: None,
                copy: false,
            },
            DropTarget::Enabled(session) => {
                let (session, report, copy) = step(session, input);
                DragStep {
                    target: DropTarget::Enabled(session),
                    report,
                    copy,
                }
            }
        }
    }

    pub fn enabled(self) -> DropTarget {
        match self {
            DropTarget::Disabled => DropTarget::Enabled(DragSession::Idle),
            enabled => enabled,
        }
    }
}
