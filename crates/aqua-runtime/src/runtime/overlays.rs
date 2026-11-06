use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use aqua_core::{NodeId, Point, Role};
use aqua_ui::{
    binding::View,
    controls::{Control, ControlRequest},
    element::{lower_specs, ui, Element},
    overlay::{CloseReason, OverlayId},
    overlay_host::{HostError, PopupRequest},
    style::{px, Axis, Style},
    tree::UiCommand,
};

use super::Runtime;

pub use aqua_ui::popup::{PopupFocus, PopupOptions};

#[derive(Debug)]
pub enum PopupError {
    Disposed,
    Content(String),
    Host(HostError),
}

impl fmt::Display for PopupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disposed => formatter.write_str("runtime is disposed"),
            Self::Content(error) => formatter.write_str(error),
            Self::Host(error) => write!(formatter, "popup host failed: {error:?}"),
        }
    }
}

impl std::error::Error for PopupError {}

pub(super) struct PendingTooltip {
    owner: NodeId,
    show_at: f64,
    tooltip: aqua_ui::popup::Tooltip,
}

pub(super) struct BoundPopup {
    pub(super) view: View,
    owner: Option<NodeId>,
    on_close: Option<Rc<dyn Fn(CloseReason)>>,
    pending: aqua_ui::popup::PendingPopup,
}

impl Runtime {
    pub fn set_popup_uncontained(
        &mut self,
        overlay: OverlayId,
        uncontained: bool,
    ) -> Option<aqua_core::Rect> {
        self.host
            .set_popup_uncontained(&mut self.tree, overlay, uncontained)
    }

    pub fn set_popup_placement(
        &mut self,
        overlay: OverlayId,
        placement: aqua_ui::overlay::Placement,
    ) -> Option<aqua_core::Rect> {
        self.host
            .set_popup_placement(&mut self.tree, overlay, placement)
    }

    pub fn set_popup_pointer_anchor(
        &mut self,
        overlay: OverlayId,
        point: aqua_core::Point,
    ) -> Option<aqua_core::Rect> {
        self.host
            .set_popup_pointer_anchor(&mut self.tree, overlay, point)
    }

    pub fn open_popup(
        &mut self,
        owner: NodeId,
        content: Element,
        options: PopupOptions,
    ) -> Result<OverlayId, PopupError> {
        self.open_bound_popup(owner, content, options, None, false)
    }

    pub fn open_popup_measured(
        &mut self,
        owner: NodeId,
        content: Element,
        options: PopupOptions,
    ) -> Result<OverlayId, PopupError> {
        self.open_bound_popup(owner, content, options, None, true)
    }

    fn open_bound_popup(
        &mut self,
        owner: NodeId,
        content: Element,
        options: PopupOptions,
        control_owner: Option<NodeId>,
        measure_content: bool,
    ) -> Result<OverlayId, PopupError> {
        if self.disposed {
            return Err(PopupError::Disposed);
        }
        let content = match options.kind {
            aqua_ui::overlay::OverlayKind::Menu => content.role(Role::Menu),
            aqua_ui::overlay::OverlayKind::Dialog => content.role(Role::Dialog),
            aqua_ui::overlay::OverlayKind::Tooltip => content.role(Role::Tooltip),
            aqua_ui::overlay::OverlayKind::Popover => content,
        };
        let mut specs =
            lower_specs(&content).map_err(|error| PopupError::Content(error.to_string()))?;
        if specs.len() != 1 {
            return Err(PopupError::Content(
                "popup content must have one root".to_owned(),
            ));
        }
        let mut spec = specs.pop().unwrap();
        if let Some(owner) = control_owner {
            spec.control = Some(Box::new(Control::SelectPopup(owner)));
        }
        let parent = self.overlay_for_node(owner);
        let request = PopupRequest {
            owner,
            parent,
            kind: options.kind,
            modality: options.modality,
            dismiss: options.dismiss,
            content: spec,
            placement: options.placement,
            autofocus: None,
            focus_root: options.focus != PopupFocus::Preserve,
        };
        let overlay = if measure_content {
            let measurer = aqua_ui::layout::ProfiledCosmicMeasurer {
                fonts: self.fonts.clone(),
                scale: self.scale,
                gpui_line_height: self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded,
                gpui_wrap: self.color_profile == aqua_core::ColorProfile::GpuiWindowsEncoded,
            };
            self.host
                .open_popup_measured(&mut self.tree, request, options.at, &measurer)
        } else {
            self.host.open_popup_at(&mut self.tree, request, options.at)
        }
        .map_err(PopupError::Host)?;
        let root = self.host.popup_root(overlay).unwrap();
        let mut view = View::new(self.runtime.create_scope(), self.tree.command_sink());
        if let Err(error) = view.bind_existing(&mut self.tree, root, content) {
            self.host
                .close_popup(&mut self.tree, overlay, CloseReason::Requested);
            view.dispose();
            return Err(PopupError::Content(error.to_string()));
        }
        if options.focus == PopupFocus::First {
            let mut stack = self
                .tree
                .get(root)
                .unwrap()
                .children
                .iter()
                .rev()
                .copied()
                .collect::<Vec<_>>();
            while let Some(node) = stack.pop() {
                let Some(record) = self.tree.get(node) else {
                    continue;
                };
                if record.focusable && record.tab_index >= 0 && !self.tree.is_disabled(node) {
                    self.tree.set_focused(Some(node));
                    break;
                }
                stack.extend(record.children.iter().rev().copied());
            }
        }
        if let Some(enter) = options.presence.enter_motion() {
            let kind = self.motion_rt.theme().resolve(enter).kind();
            self.animate_popup(
                overlay,
                aqua_ui::overlay_host::PopupMotion {
                    property: aqua_motion::Property::Opacity,
                    from: aqua_motion::MotionValue::Scalar(0.0),
                    to: aqua_motion::MotionValue::Scalar(1.0),
                    kind,
                },
            );
            if let Some((dx, dy)) = options.presence.slide_offset() {
                self.animate_popup(
                    overlay,
                    aqua_ui::overlay_host::PopupMotion {
                        property: aqua_motion::Property::Translate,
                        from: aqua_motion::MotionValue::Pair(dx, dy),
                        to: aqua_motion::MotionValue::Pair(0.0, 0.0),
                        kind,
                    },
                );
            }
        }
        if let Some(exit) = options.presence.exit_motion() {
            let kind = self.motion_rt.theme().resolve(exit).kind();
            self.host.set_popup_exit(
                overlay,
                aqua_ui::overlay_host::PopupExit {
                    kind,
                    slide: options.presence.slide_offset(),
                },
            );
        }
        let pending = Rc::new(RefCell::new(None));
        if let Some(reactive) = options.reactive {
            reactive.attach(view.scope(), pending.clone());
        }
        self.bound_popups.insert(
            overlay,
            BoundPopup {
                view,
                owner: control_owner,
                on_close: options.on_close,
                pending,
            },
        );
        Ok(overlay)
    }

    pub fn refresh_popup(
        &mut self,
        overlay: OverlayId,
        content: Element,
    ) -> Result<(), PopupError> {
        if self.disposed {
            return Err(PopupError::Disposed);
        }
        let Some(popup) = self.bound_popups.get_mut(&overlay) else {
            return Err(PopupError::Content("popup is closed".to_owned()));
        };
        let specs =
            lower_specs(&content).map_err(|error| PopupError::Content(error.to_string()))?;
        let root = popup
            .view
            .root()
            .ok_or_else(|| PopupError::Content("popup root is absent".to_owned()))?;
        if specs.len() != 1
            || self.tree.get(root).is_none_or(|record| {
                std::mem::discriminant(&record.kind) != std::mem::discriminant(&specs[0].kind)
            })
        {
            return Err(PopupError::Content(
                "popup refresh must preserve its root kind".to_owned(),
            ));
        }
        let content = content.role(self.tree.get(root).unwrap().semantics.role);
        popup
            .view
            .set_content(&mut self.tree, vec![content])
            .map_err(|error| PopupError::Content(error.to_string()))?;
        self.tree.drain_commands();
        self.host.reanchor_popup(&mut self.tree, overlay);
        Ok(())
    }

    pub(super) fn refresh_reactive_popups(&mut self) -> Result<(), String> {
        let updates: Vec<_> = self
            .bound_popups
            .iter()
            .filter_map(|(overlay, popup)| {
                popup
                    .pending
                    .borrow_mut()
                    .take()
                    .map(|content| (*overlay, content))
            })
            .collect();
        for (overlay, content) in updates {
            self.refresh_popup(overlay, content)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn close_popup(&mut self, overlay: OverlayId) -> bool {
        let closed = self
            .host
            .close_popup(&mut self.tree, overlay, CloseReason::Requested);
        self.sync_bound_popups();
        !closed.is_empty()
    }

    fn overlay_for_node(&self, mut node: NodeId) -> Option<OverlayId> {
        loop {
            if let Some(overlay) = self.host.overlay_of_root(node) {
                return Some(overlay);
            }
            node = self.tree.get(node)?.parent?;
        }
    }

    pub(super) fn sync_bound_popups(&mut self) {
        self.host.sync_lifecycle(&mut self.tree);
        for closed in self.host.take_closed() {
            if let Some(popup) = self.bound_popups.remove(&closed.id) {
                popup.view.dispose();
                if self
                    .tooltip_popup
                    .is_some_and(|(_, overlay)| overlay == closed.id)
                {
                    self.tooltip_popup = None;
                }
                if let Some(owner) = popup.owner {
                    self.select_popups.remove(&owner);
                    self.tree.close_control_select(owner);
                }
                if let Some(callback) = popup.on_close {
                    callback(closed.reason);
                }
            }
        }
    }

    pub(super) fn process_control_requests(&mut self) {
        self.sync_bound_popups();
        for request in self.tree.take_control_requests() {
            match request {
                ControlRequest::OpenPopup {
                    owner,
                    content,
                    options,
                } => {
                    if self.host.input_allowed(&self.tree, owner) && !self.tree.is_disabled(owner) {
                        if let Err(error) = self.open_popup(owner, *content, options) {
                            self.last_build_error = Some(error.to_string());
                        }
                    }
                }
                ControlRequest::ArmTooltip { owner, tooltip } => {
                    if !self
                        .tooltip_pending
                        .as_ref()
                        .is_some_and(|pending| pending.owner == owner)
                        && !self
                            .tooltip_popup
                            .is_some_and(|(current, _)| current == owner)
                    {
                        self.dismiss_tooltip();
                        let delay = if tooltip.delay.is_finite() {
                            tooltip.delay.max(0.0)
                        } else {
                            0.5
                        };
                        self.tooltip_pending = Some(PendingTooltip {
                            owner,
                            show_at: self.clock + delay,
                            tooltip,
                        });
                    }
                }
                ControlRequest::HideTooltip { owner } => {
                    if self
                        .tooltip_pending
                        .as_ref()
                        .is_some_and(|pending| pending.owner == owner)
                        || self
                            .tooltip_popup
                            .is_some_and(|(current, _)| current == owner)
                    {
                        self.dismiss_tooltip();
                    }
                }

                ControlRequest::OpenSelect { owner } => {
                    if self.select_popups.contains_key(&owner) {
                        continue;
                    }
                    let Some(select) = self.tree.select_options(owner) else {
                        continue;
                    };
                    let options = select.options.clone();
                    let highlighted = select.highlighted();
                    let sink = self.tree.command_sink();
                    let width = self
                        .tree
                        .rect(owner)
                        .map_or(240.0, |rect| rect.w.max(120.0));
                    let height =
                        (options.len() as f32 * 32.0).min((self.viewport.h - 16.0).max(32.0));
                    let background = self
                        .tree
                        .get(owner)
                        .and_then(|record| record.paint().fill)
                        .unwrap_or(aqua_core::Oklch::new(0.24, 0.008, 255.0));
                    let foreground = self
                        .tree
                        .get(owner)
                        .and_then(|record| record.children.first())
                        .and_then(|node| self.tree.get(*node))
                        .and_then(|record| match &record.kind {
                            aqua_ui::tree::NodeKind::Text(text) => Some(text.color),
                            _ => None,
                        })
                        .unwrap_or(aqua_core::Oklch::WHITE);
                    let items = options
                        .iter()
                        .enumerate()
                        .map(|(index, option)| {
                            let sink = sink.clone();
                            ui::Button::new()
                                .style(
                                    Style::new()
                                        .width(px(width))
                                        .height(px(32.0))
                                        .padding(px(7.0))
                                        .focus_ring(1.5, foreground)
                                        .flex_shrink(0.0),
                                )
                                .on_activate(move || {
                                    sink.push(UiCommand::ChooseOption { node: owner, index })
                                })
                                .child(
                                    ui::Text::builder()
                                        .arg(option.label.clone())
                                        .style(Style::new().text_color(if option.disabled {
                                            foreground.lerp(background, 0.55)
                                        } else {
                                            foreground
                                        }))
                                        .build(),
                                )
                                .build()
                                .role(Role::MenuItem)
                                .disabled(option.disabled)
                                .aria_label(option.label.clone())
                        })
                        .collect::<Vec<_>>();
                    let content = ui::Scroll::new()
                        .style(
                            Style::new()
                                .width(px(width))
                                .height(px(height))
                                .background(background)
                                .axis(Axis::Column),
                        )
                        .children(items)
                        .build()
                        .role(Role::Menu);
                    match self.open_bound_popup(
                        owner,
                        content,
                        PopupOptions::menu(),
                        Some(owner),
                        false,
                    ) {
                        Ok(overlay) => {
                            self.select_popups.insert(owner, overlay);
                            if let Some(index) = highlighted {
                                self.highlight_select(owner, index);
                            }
                        }
                        Err(error) => {
                            self.last_build_error = Some(error.to_string());
                            self.tree.close_control_select(owner);
                        }
                    }
                }
                ControlRequest::CloseSelect { owner } => {
                    if let Some(overlay) = self.select_popups.get(&owner).copied() {
                        self.close_popup(overlay);
                    }
                }
                ControlRequest::HighlightSelect { owner, index } => {
                    self.highlight_select(owner, index)
                }
            }
        }
        self.advance_tooltip();
    }

    pub(super) fn dismiss_tooltip(&mut self) {
        self.tooltip_pending = None;
        if let Some((_, popup)) = self.tooltip_popup.take() {
            self.close_popup(popup);
        }
    }

    fn advance_tooltip(&mut self) {
        let ready = self
            .tooltip_pending
            .as_ref()
            .is_some_and(|pending| pending.show_at <= self.clock);
        if ready {
            let pending = self.tooltip_pending.take().unwrap();
            if self.tree.get(pending.owner).is_some()
                && !self.tree.is_disabled(pending.owner)
                && self.host.input_allowed(&self.tree, pending.owner)
            {
                let pointer = pending
                    .tooltip
                    .pointer_anchored
                    .then(|| self.tree.pointer_position())
                    .flatten();
                let result = if let Some(pointer) = pointer {
                    self.open_popup_measured(
                        pending.owner,
                        (pending.tooltip.content)(),
                        PopupOptions::tooltip().at(pointer),
                    )
                } else {
                    self.open_popup(
                        pending.owner,
                        (pending.tooltip.content)(),
                        PopupOptions::tooltip(),
                    )
                };
                match result {
                    Ok(overlay) if pointer.is_some() => {
                        self.host.set_pointer_tooltip(&mut self.tree, overlay);
                        self.tooltip_popup = Some((pending.owner, overlay));
                    }
                    Ok(overlay) => self.tooltip_popup = Some((pending.owner, overlay)),
                    Err(error) => self.last_build_error = Some(error.to_string()),
                }
            }
        }
    }

    pub(super) fn tooltip_deadline(&self) -> Option<f64> {
        self.tooltip_pending.as_ref().map(|pending| pending.show_at)
    }

    fn highlight_select(&mut self, owner: NodeId, index: usize) {
        let Some(root) = self
            .select_popups
            .get(&owner)
            .and_then(|overlay| self.host.popup_root(*overlay))
        else {
            return;
        };
        let Some(children) = self.tree.get(root).map(|record| record.children.clone()) else {
            return;
        };
        for (position, child) in children.iter().copied().enumerate() {
            if let Some(record) = self.tree.get_mut(child) {
                record.semantics.selected = position == index;
            }
        }
        if let Some(child) = children.get(index).copied() {
            self.tree.set_focused(Some(child));
            if let (Some(row), Some(viewport)) = (self.tree.rect(child), self.tree.rect(root)) {
                let mut offset = self.tree.scroll_offset(root).unwrap_or(Point::ZERO);
                let top = row.y - viewport.y;
                if top < offset.y {
                    offset.y = top;
                }
                if top + row.h > offset.y + viewport.h {
                    offset.y = top + row.h - viewport.h;
                }
                self.tree.scroll_to(root, offset);
            }
        }
    }
}
