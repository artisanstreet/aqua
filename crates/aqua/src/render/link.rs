use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use aqua_core::Size;
use aqua_core::scene::TextRasterMode;
use aqua_protocol::{RenderCommand, RenderEvent, SceneDiff};
use aqua_runtime::{RenderSide, emit_scene_diff};
use aqua_scene::Scene;
use aqua_transport::{Notify, Sender as RingSender, channel_with_notify};

use super::host::{Clock, RenderStats, TargetInit};
use super::service::{NewSlot, RenderService, ServiceHandle};
use super::transition::Settings;

const RING_CAPACITY: usize = 4096;

#[derive(Clone, Debug)]
pub enum ExternalUpdate {
    Image {
        layer: u64,
        width: u32,
        height: u32,
        rgba: Arc<[u8]>,
    },
    Release {
        layer: u64,
    },
    Transition(Settings),
    TextRaster(TextRasterMode),
}

pub type CompositionSink = Arc<dyn Fn(Box<dyn Any + Send>) + Send + Sync>;

pub struct RenderContext {
    pub externals: Receiver<ExternalUpdate>,
    pub stats: Arc<RenderStats>,
    pub composition: CompositionSink,
}

pub type InitFactory = Box<dyn FnOnce(RenderContext) -> TargetInit + Send>;

pub struct RenderLink {
    service: ServiceHandle,
    slot: u64,
    commands: RingSender<RenderCommand>,
    events: aqua_transport::Receiver<RenderEvent>,
    backlog: VecDeque<RenderCommand>,
    externals: Sender<ExternalUpdate>,
    viewport: Option<(Size, f32)>,
    stats: Arc<RenderStats>,
    submitted: u64,
    scenes_queued: u64,
}

impl RenderLink {
    pub fn spawn(
        service: &RenderService,
        factory: InitFactory,
        composition: CompositionSink,
        clock: Clock,
        interval: Duration,
        wake: Notify,
    ) -> Self {
        let (externals, receiver) = channel();
        let stats = Arc::new(RenderStats::default());
        let init = factory(RenderContext {
            externals: receiver,
            stats: stats.clone(),
            composition,
        });
        let handle = service.handle();
        let (commands, command_rx) = channel_with_notify(RING_CAPACITY, handle.notifier());
        let (event_tx, events) = channel_with_notify(RING_CAPACITY, wake);
        let slot = handle.add(NewSlot {
            commands: command_rx,
            events: event_tx,
            init,
            clock,
            interval,
            stats: stats.clone(),
        });
        Self {
            service: handle,
            slot,
            commands,
            events,
            backlog: VecDeque::new(),
            externals,
            viewport: None,
            stats,
            submitted: 0,
            scenes_queued: 0,
        }
    }

    pub fn set_immediate(&self, immediate: bool) {
        self.stats
            .immediate
            .store(immediate, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn request_trim(&self) {
        self.stats
            .trim
            .store(true, std::sync::atomic::Ordering::Release);
        self.service.wake();
    }

    pub fn set_hidden(&self, hidden: bool) {
        self.stats
            .hidden
            .store(hidden, std::sync::atomic::Ordering::Release);
        self.service.wake();
    }

    pub fn inject_device_loss(&self) {
        self.stats
            .inject_loss
            .store(true, std::sync::atomic::Ordering::Release);
        self.service.wake();
    }

    pub fn stats(&self) -> &Arc<RenderStats> {
        &self.stats
    }

    pub fn submitted(&self) -> u64 {
        self.submitted
    }

    pub fn scenes_queued(&self) -> u64 {
        self.scenes_queued
    }

    pub fn backlog(&self) -> usize {
        self.backlog.len()
    }

    pub fn set_viewport(&mut self, size: Size, scale: f32) {
        if self.viewport != Some((size, scale)) {
            self.viewport = Some((size, scale));
            self.backlog
                .push_back(RenderCommand::Viewport { size, scale });
            self.flush();
        }
    }

    pub fn push_scene(&mut self, scene: &mut Scene) -> bool {
        let diff = emit_scene_diff(scene);
        match diff.is_empty() {
            true => false,
            false => {
                self.backlog.push_back(RenderCommand::Scene(diff));
                self.submitted += 1;
                self.scenes_queued += 1;
                self.flush();
                true
            }
        }
    }

    pub fn push_diff(&mut self, diff: SceneDiff) {
        if !diff.is_empty() {
            self.backlog.push_back(RenderCommand::Scene(diff));
            self.submitted += 1;
            self.scenes_queued += 1;
        }
        self.flush();
    }

    pub fn push_commands(&mut self, commands: impl IntoIterator<Item = RenderCommand>) {
        for command in commands {
            self.scenes_queued += u64::from(matches!(command, RenderCommand::Scene(_)));
            self.backlog.push_back(command);
        }
        self.flush();
    }

    pub fn external(&self, update: ExternalUpdate) {
        if self.externals.send(update).is_ok() {
            self.stats
                .arrivals
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            self.service.wake();
        }
    }

    pub fn flush(&mut self) {
        while let Some(command) = self.backlog.pop_front() {
            match self.commands.try_send(command) {
                Ok(()) => {}
                Err(aqua_transport::SendError::Full(command))
                | Err(aqua_transport::SendError::Disconnected(command)) => {
                    self.backlog.push_front(command);
                    break;
                }
            }
        }
    }

    pub fn poll(&mut self) -> Vec<RenderEvent> {
        self.flush();
        self.events.drain(RING_CAPACITY)
    }

    pub fn wait_event(&mut self, timeout: Duration) -> Option<RenderEvent> {
        self.events.recv_timeout(Some(timeout)).ok()
    }

    pub fn shutdown(&mut self) -> Option<RenderSide> {
        self.service.remove(self.slot)
    }
}

impl Drop for RenderLink {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
