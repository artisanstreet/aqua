use crate::looping::emit;
use crate::model::drop::{DragInput, DropTarget};
use crate::view::AquaView;
use aqua_core::Point;
use aqua_shell::ShellEvent;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSDragOperation, NSDraggingInfo};
use std::path::PathBuf;

fn position(view: &AquaView, info: &ProtocolObject<dyn NSDraggingInfo>) -> Point {
    let local = view.convertPoint_fromView(info.draggingLocation(), None);
    Point::new(local.x as f32, local.y as f32)
}

fn paths(info: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    crate::clipboard::file_paths(&info.draggingPasteboard())
}

fn target<T>(view: &AquaView, read: impl FnOnce(&mut DropTarget) -> T) -> T {
    let id = view.id();
    crate::state::window(id, |window| read(&mut window.drop))
        .unwrap_or_else(|| panic!("invariant: window {id:?} receives drags only while it lives"))
}

fn advance(view: &AquaView, input: DragInput) -> bool {
    let id = view.id();
    let step = target(view, |drop| {
        let step = std::mem::take(drop).step(input);
        *drop = step.target.clone();
        step
    });
    if let Some(report) = step.report {
        emit(id, ShellEvent::FileDrag(report));
    }
    step.copy
}

fn operation(copy: bool) -> NSDragOperation {
    match copy {
        true => NSDragOperation::Copy,
        false => NSDragOperation::None,
    }
}

fn accepting(view: &AquaView) -> bool {
    target(view, |drop| *drop != DropTarget::Disabled)
}

pub(crate) fn entered(
    view: &AquaView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
) -> NSDragOperation {
    let paths = match accepting(view) {
        true => paths(info),
        false => Vec::new(),
    };
    let position = position(view, info);
    operation(advance(view, DragInput::Entered { paths, position }))
}

pub(crate) fn updated(
    view: &AquaView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
) -> NSDragOperation {
    let position = position(view, info);
    operation(advance(view, DragInput::Updated { position }))
}

pub(crate) fn exited(view: &AquaView) {
    advance(view, DragInput::Exited);
}

pub(crate) fn perform(view: &AquaView, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
    let position = position(view, info);
    advance(view, DragInput::Performed { position })
}
