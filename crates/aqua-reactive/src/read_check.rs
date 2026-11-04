use std::cell::RefCell;
use std::panic::Location;

use crate::graph::NodeId;
use crate::signals::DepTuple;

enum Frame {
    Render(Vec<NodeId>),
    Unchecked,
}

thread_local! {
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

struct Pop;

impl Drop for Pop {
    fn drop(&mut self) {
        FRAMES.with(|frames| {
            frames.borrow_mut().pop();
        });
    }
}

fn scoped<R>(frame: Frame, body: impl FnOnce() -> R) -> R {
    FRAMES.with(|frames| frames.borrow_mut().push(frame));
    let _pop = Pop;
    body()
}

pub fn render_scope<D: DepTuple, R>(deps: &D, render: impl FnOnce() -> R) -> R {
    scoped(Frame::Render(deps.ids()), render)
}

pub fn unchecked<R>(body: impl FnOnce() -> R) -> R {
    scoped(Frame::Unchecked, body)
}

#[must_use]
pub fn in_render() -> bool {
    FRAMES.with(|frames| matches!(frames.borrow().last(), Some(Frame::Render(_))))
}

#[cfg(debug_assertions)]
pub(crate) fn note_read(node: NodeId, created_at: &'static Location<'static>) {
    let undeclared = FRAMES.with(|frames| match frames.borrow().last() {
        Some(Frame::Render(declared)) => !declared.contains(&node),
        Some(Frame::Unchecked) | None => false,
    });
    if undeclared {
        panic!(
            "aqua-reactive: signal {node:?} created at {created_at} was read during render \
             but is not in the declared dependency tuple"
        );
    }
}

#[cfg(not(debug_assertions))]
pub(crate) fn note_read(_node: NodeId, _created_at: &'static Location<'static>) {}
