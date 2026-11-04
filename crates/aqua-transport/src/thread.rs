use std::thread::{self, JoinHandle};
use std::time::Duration;

use aqua_protocol::{
    Feedback, FrameId, Instant, RenderCommand, RenderEvent, RenderInput, RenderWake,
};
use aqua_runtime::RenderSide;

use crate::ring::{channel_with_notify, Notify, Receiver, RecvError, Sender};

const DRAIN_LIMIT: usize = 4096;

pub trait RenderHost: 'static {
    fn present_time(&mut self) -> Instant;
    fn wait_frame(&mut self);
    fn execute(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback>;
}

pub struct RenderThread {
    commands: Sender<RenderCommand>,
    events: Receiver<RenderEvent>,
    handle: JoinHandle<RenderSide>,
}

impl RenderThread {
    pub fn send(&mut self, command: RenderCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    pub fn send_all(&mut self, commands: impl IntoIterator<Item = RenderCommand>) -> bool {
        commands.into_iter().all(|command| self.send(command))
    }

    pub fn poll(&mut self) -> Vec<RenderEvent> {
        self.events.drain(DRAIN_LIMIT)
    }

    pub fn wait_event(&mut self, timeout: Duration) -> Option<RenderEvent> {
        self.events.recv_timeout(Some(timeout)).ok()
    }

    pub fn shutdown(self) -> Option<RenderSide> {
        let Self {
            commands,
            events,
            handle,
        } = self;
        drop(commands);
        let side = handle.join().ok();
        drop(events);
        side
    }
}

pub fn spawn_render_thread<H: RenderHost + Send>(
    capacity: usize,
    host: H,
    wake_app: Notify,
) -> RenderThread {
    spawn_render_thread_with(capacity, move || host, wake_app)
}

pub fn spawn_render_thread_with<H: RenderHost>(
    capacity: usize,
    build: impl FnOnce() -> H + Send + 'static,
    wake_app: Notify,
) -> RenderThread {
    let (command_tx, mut command_rx) = crate::ring::channel::<RenderCommand>(capacity);
    let (mut event_tx, event_rx) = channel_with_notify::<RenderEvent>(capacity, wake_app);
    let handle = thread::spawn(move || {
        let mut host = build();
        let mut side = RenderSide::new();
        let mut feedback: Vec<Feedback> = Vec::new();
        let mut wake = RenderWake::NextFrame;
        loop {
            let mut commands = command_rx.drain(DRAIN_LIMIT);
            let idle = commands.is_empty() && feedback.is_empty();
            match (idle, wake) {
                (true, RenderWake::Idle) => match command_rx.recv_timeout(None) {
                    Ok(command) => commands.push(command),
                    Err(RecvError::Disconnected) => break,
                    Err(_) => continue,
                },
                (true, RenderWake::At(at)) => {
                    let remaining = at.saturating_duration_since(host.present_time());
                    match command_rx.recv_timeout(Some(remaining)) {
                        Ok(command) => commands.push(command),
                        Err(RecvError::Disconnected) => break,
                        Err(_) => {}
                    }
                }
                (true, RenderWake::NextFrame) | (false, _) => {}
            }
            host.wait_frame();
            commands.extend(command_rx.drain(DRAIN_LIMIT));
            let input = RenderInput {
                commands,
                feedback: std::mem::take(&mut feedback),
                present_at: host.present_time(),
            };
            let output = side.step(input);
            wake = output.wake;
            for event in output.events {
                if event_tx.send(event).is_err() {
                    return side;
                }
            }
            if let Some(frame) = output.frame {
                feedback = host.execute(&mut side, frame);
            }
        }
        side
    });
    RenderThread {
        commands: command_tx,
        events: event_rx,
        handle,
    }
}
