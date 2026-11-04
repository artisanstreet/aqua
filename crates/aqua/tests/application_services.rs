use aqua::mailbox::Mailbox;
use aqua::{AppCtx, Application, Aqua, AquaConfig, Instant};
use aqua_core::InputEvent;
use aqua_protocol::geometry::AquaWindowId;
use aqua_protocol::geometry::LogicalSize;
use aqua_protocol::window::ObservedWindowState;
use aqua_protocol::window::WindowError;
use aqua_protocol::window::WindowEvent;
use aqua_protocol::window::WindowOptions;
use aqua_protocol::window::WindowRequest;
use aqua_protocol::FrameReason;
use std::num::NonZeroUsize;

struct Consumer {
    messages: Mailbox<String>,
    received: Vec<String>,
    inputs: Vec<InputEvent>,
    started: Vec<AquaWindowId>,
    errors: Vec<WindowError>,
    allow_close: bool,
    initial_request: Option<WindowRequest>,
}

impl Application for Consumer {
    fn on_start(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
        self.started.push(id);
        ctx.bind_mailbox(id, &self.messages);
        if let Some(request) = self.initial_request.take() {
            ctx.request_window(request).unwrap();
        }
    }

    fn on_wake(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
        let messages = self.messages.drain();
        if !messages.is_empty() {
            self.received.extend(messages);
            ctx.request_frame(id, FrameReason::MEDIA);
        }
    }

    fn on_window_event(&mut self, _: &mut AppCtx, _: AquaWindowId, _: &WindowEvent) {}

    fn on_input(&mut self, _: &mut AppCtx, _: AquaWindowId, event: &InputEvent) {
        self.inputs.push(event.clone());
    }

    fn on_window_request_error(&mut self, _: &WindowRequest, error: &WindowError) {
        self.errors.push(error.clone());
    }

    fn can_close(&mut self, _: &mut AppCtx, _: AquaWindowId) -> bool {
        self.allow_close
    }

    fn build_scene(&mut self, _: &ObservedWindowState, _: Instant) -> Option<Instant> {
        None
    }
}

fn setup(request: Option<WindowRequest>) -> (Aqua, Consumer, AquaWindowId) {
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let id = AquaWindowId::new(5).unwrap();
    aqua.add_headless_window(ObservedWindowState {
        id,
        title: "Editor".into(),
        inner_size: LogicalSize::new(1000.0, 700.0),
        scale_factor: 1.0,
        focused: false,
        visible: false,
        occluded: false,
        maximized: false,
    });
    let consumer = Consumer {
        messages: Mailbox::new(NonZeroUsize::new(16).unwrap()),
        received: Vec::new(),
        inputs: Vec::new(),
        started: Vec::new(),
        errors: Vec::new(),
        allow_close: false,
        initial_request: request,
    };
    (aqua, consumer, id)
}

#[test]
fn background_delivery_is_applied_at_the_window_boundary_and_stale_wakes_are_ignored() {
    let (mut aqua, mut consumer, id) = setup(None);
    aqua.dispatch_start(&mut consumer, id);
    let sender = consumer.messages.sender();
    std::thread::spawn(move || sender.send("audio progress".into()).unwrap())
        .join()
        .unwrap();
    assert!(consumer.received.is_empty());
    aqua.dispatch_wake(&mut consumer, id);
    assert_eq!(consumer.received, ["audio progress"]);
    consumer.allow_close = true;
    aqua.dispatch_window_event(&mut consumer, id, &WindowEvent::CloseRequested(id));
    consumer
        .messages
        .sender()
        .send("late worker".into())
        .unwrap();
    aqua.dispatch_wake(&mut consumer, id);
    assert_eq!(consumer.received, ["audio progress"]);
}

#[test]
fn window_creation_allocates_fresh_identity_and_close_can_be_vetoed() {
    let (mut aqua, mut consumer, id) = setup(Some(WindowRequest::Create(WindowOptions::default())));
    aqua.dispatch_start(&mut consumer, id);
    aqua.dispatch_window_requests(&mut consumer);
    assert_eq!(consumer.started.len(), 2);
    assert_ne!(consumer.started[0], consumer.started[1]);
    aqua.dispatch_window_event(&mut consumer, id, &WindowEvent::CloseRequested(id));
    aqua.dispatch_start(&mut consumer, id);
    assert_eq!(consumer.started.len(), 3);
    consumer.allow_close = true;
    aqua.dispatch_window_event(&mut consumer, id, &WindowEvent::CloseRequested(id));
    aqua.dispatch_start(&mut consumer, id);
    assert_eq!(consumer.started.len(), 3);
}

#[test]
fn stale_window_requests_report_errors_instead_of_targeting_another_window() {
    let missing = AquaWindowId::new(99).unwrap();
    let (mut aqua, mut consumer, id) = setup(Some(WindowRequest::Activate(missing)));
    aqua.dispatch_start(&mut consumer, id);
    aqua.dispatch_window_requests(&mut consumer);
    assert_eq!(consumer.errors, [WindowError::UnknownWindow(missing)]);
}

#[test]
fn native_window_focus_occurrences_reach_input_once_and_stale_events_are_ignored() {
    let (mut aqua, mut consumer, id) = setup(None);
    aqua.dispatch_window_event(&mut consumer, id, &WindowEvent::Focused(id, true));
    aqua.dispatch_window_event(&mut consumer, id, &WindowEvent::Focused(id, false));
    assert_eq!(
        consumer.inputs,
        [InputEvent::Focused(true), InputEvent::Focused(false)]
    );
    consumer.allow_close = true;
    aqua.dispatch_window_event(&mut consumer, id, &WindowEvent::CloseRequested(id));
    aqua.dispatch_window_event(&mut consumer, id, &WindowEvent::Focused(id, true));
    aqua.dispatch_input(&mut consumer, id, &InputEvent::Focused(true));
    assert_eq!(consumer.inputs.len(), 2);
}
