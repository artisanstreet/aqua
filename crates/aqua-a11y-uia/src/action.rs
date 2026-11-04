use accesskit::{Action, ActionData};
use aqua_protocol::{A11yAction, A11yActionData, A11yActionRequest};

use crate::tree::unpack_node_id;

fn map_action(action: &Action) -> A11yAction {
    match action {
        Action::Click => A11yAction::Press,
        Action::Focus => A11yAction::Focus,
        Action::Expand => A11yAction::Expand,
        Action::Collapse => A11yAction::Collapse,
        Action::Increment => A11yAction::Increment,
        Action::Decrement => A11yAction::Decrement,
        Action::Blur => A11yAction::Other("blur"),
        Action::CustomAction => A11yAction::Other("custom"),
        Action::HideTooltip => A11yAction::Other("hide-tooltip"),
        Action::ShowTooltip => A11yAction::Other("show-tooltip"),
        Action::ReplaceSelectedText => A11yAction::ReplaceSelectedText,
        Action::ScrollDown
        | Action::ScrollLeft
        | Action::ScrollRight
        | Action::ScrollUp
        | Action::ScrollIntoView
        | Action::ScrollToPoint
        | Action::SetScrollOffset => A11yAction::Other("scroll"),
        Action::SetTextSelection => A11yAction::Other("set-text-selection"),
        Action::SetSequentialFocusNavigationStartingPoint => {
            A11yAction::Other("sequential-focus-start")
        }
        Action::SetValue => A11yAction::SetValue,
        Action::ShowContextMenu => A11yAction::Other("context-menu"),
    }
}

pub(crate) fn map_action_data(data: Option<&ActionData>) -> A11yActionData {
    match data {
        None => A11yActionData::None,
        Some(ActionData::Value(v)) => A11yActionData::Value(v.to_string()),
        Some(ActionData::NumericValue(n)) => A11yActionData::Numeric(*n),
        Some(ActionData::SetTextSelection(sel)) => A11yActionData::TextSelection {
            anchor_node: unpack_node_id(sel.anchor.node),
            anchor_index: sel.anchor.character_index,
            focus_node: unpack_node_id(sel.focus.node),
            focus_index: sel.focus.character_index,
        },
        Some(ActionData::ScrollToPoint(p)) => A11yActionData::Point { x: p.x, y: p.y },
        Some(ActionData::SetScrollOffset(p)) => A11yActionData::ScrollOffset { x: p.x, y: p.y },
        Some(ActionData::CustomAction(id)) => A11yActionData::Custom(*id),
        Some(ActionData::ScrollUnit(_)) | Some(ActionData::ScrollHint(_)) => A11yActionData::None,
    }
}

pub(crate) fn translate_action_request(request: &accesskit::ActionRequest) -> A11yActionRequest {
    A11yActionRequest {
        node: unpack_node_id(request.target_node),
        action: map_action(&request.action),
        data: map_action_data(request.data.as_ref()),
    }
}
