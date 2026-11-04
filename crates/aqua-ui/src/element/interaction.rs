use crate::style::Style;
use crate::tree::interaction::InteractionStyleKind;

use super::{Element, ElementBinding, TypoChain};

impl Element {
    pub fn gpui_keyboard_activation(mut self) -> Self {
        self.bindings
            .push(ElementBinding::KeyboardReleaseActivation);
        self
    }

    pub fn hover_style(mut self, style: Style) -> Self {
        self.bindings.push(ElementBinding::InteractionStyle {
            kind: InteractionStyleKind::Hover,
            style: std::boxed::Box::new(style),
        });
        self
    }

    pub fn focus_visible_style(mut self, style: Style) -> Self {
        self.bindings.push(ElementBinding::InteractionStyle {
            kind: InteractionStyleKind::FocusVisible,
            style: std::boxed::Box::new(style),
        });
        self
    }

    pub fn pressed_style(mut self, style: Style) -> Self {
        self.bindings.push(ElementBinding::InteractionStyle {
            kind: InteractionStyleKind::Pressed,
            style: std::boxed::Box::new(style),
        });
        self
    }

    pub fn aria_expanded(mut self, expanded: bool) -> Self {
        self.bindings.push(ElementBinding::Expanded(expanded));
        self
    }
}

impl Element {
    pub fn aria_expanded_signal(mut self, source: aqua_reactive::ReadSignal<bool>) -> Self {
        self.bindings.push(ElementBinding::InteractionBool {
            property: crate::tree::interaction::InteractionBoolProperty::Expanded,
            source,
        });
        self
    }

    pub fn aria_selected_signal(mut self, source: aqua_reactive::ReadSignal<bool>) -> Self {
        self.bindings.push(ElementBinding::InteractionBool {
            property: crate::tree::interaction::InteractionBoolProperty::Selected,
            source,
        });
        self
    }

    pub fn aria_checked_signal(mut self, source: aqua_reactive::ReadSignal<bool>) -> Self {
        self.bindings.push(ElementBinding::InteractionBool {
            property: crate::tree::interaction::InteractionBoolProperty::Checked,
            source,
        });
        self
    }

    pub fn aria_value_signal(mut self, source: aqua_reactive::ReadSignal<String>) -> Self {
        self.bindings.push(ElementBinding::InteractionValue(source));
        self
    }
}

pub(crate) fn bind_interaction_bool(
    scope: &aqua_reactive::Scope,
    sink: &crate::tree::CommandSink,
    node: aqua_core::NodeId,
    property: crate::tree::interaction::InteractionBoolProperty,
    source: aqua_reactive::ReadSignal<bool>,
) {
    use crate::tree::interaction::{
        InteractionBoolProperty, InteractionCommand, InteractionSemantic,
    };
    let enqueue = {
        let sink = sink.clone();
        move |value| {
            let value = match property {
                InteractionBoolProperty::Expanded => InteractionSemantic::Expanded(Some(value)),
                InteractionBoolProperty::Selected => InteractionSemantic::Selected(value),
                InteractionBoolProperty::Checked => InteractionSemantic::Checked(Some(value)),
            };
            sink.interaction(InteractionCommand::Semantics { node, value });
        }
    };
    if let Some(value) = source.try_get() {
        enqueue(value);
    }
    scope.subscribe(&source, enqueue);
}

pub(crate) fn bind_interaction_value(
    scope: &aqua_reactive::Scope,
    sink: &crate::tree::CommandSink,
    node: aqua_core::NodeId,
    source: aqua_reactive::ReadSignal<String>,
) {
    use crate::tree::interaction::{InteractionCommand, InteractionSemantic};
    let enqueue = {
        let sink = sink.clone();
        move |value| {
            sink.interaction(InteractionCommand::Semantics {
                node,
                value: InteractionSemantic::Value(Some(value)),
            });
        }
    };
    if let Some(value) = source.try_get() {
        enqueue(value);
    }
    scope.subscribe(&source, enqueue);
}

impl TypoChain {
    pub(crate) fn explicit_color(&self) -> Option<aqua_core::Oklch> {
        self.color
    }

    pub(crate) fn explicit_gpui_color(&self) -> Option<Option<aqua_core::GpuiHsla>> {
        self.gpui_color
    }

    pub(crate) fn explicit_gpui_source_color(&self) -> Option<Option<aqua_core::GpuiTextColor>> {
        self.gpui_source_color
    }
}
