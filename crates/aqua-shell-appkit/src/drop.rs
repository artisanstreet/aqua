use aqua_protocol::WindowId;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSDragOperation, NSDraggingInfo};

pub(crate) fn entered(
    _id: WindowId,
    _info: &ProtocolObject<dyn NSDraggingInfo>,
) -> NSDragOperation {
    NSDragOperation::None
}

pub(crate) fn updated(
    _id: WindowId,
    _info: &ProtocolObject<dyn NSDraggingInfo>,
) -> NSDragOperation {
    NSDragOperation::None
}

pub(crate) fn exited(_id: WindowId) {}

pub(crate) fn perform(_id: WindowId, _info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
    false
}
