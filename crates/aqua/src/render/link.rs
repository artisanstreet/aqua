use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Duration;

use aqua_core::Size;
use aqua_protocol::{RenderCommand, RenderEvent, SceneDiff};
use aqua_runtime::{emit_scene_diff, RenderSide};
use aqua_scene::Scene;
use aqua_transport::{spawn_render_thread, Notify, RenderThread};

use super::host::{Clock, DeferredHost, RenderStats, TargetInit};

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
}

pub struct RenderLink {
    thread: Option<RenderThread>,
    backlog: VecDeque<RenderCommand>,
    externals: Sender<ExternalUpdate>,
    viewport: Option<(Size, f32)>,
    stats: Arc<RenderStats>,
    submitted: u64,
}

pub type InitFactory = Box<
    dyn FnOnce(std::sync::mpsc::Receiver<ExternalUpdate>, Arc<RenderStats>) -> TargetInit + Send,
>;

impl RenderLink {
    pub fn spawn(factory: InitFactory, clock: Clock, interval: Duration, wake: Notify) -> Self {
        let (externals, receiver) = channel();
        let stats = Arc::new(RenderStats::default());
        let init = factory(receiver, stats.clone());
        let host = DeferredHost::new(init, clock, interval, Some(stats.clone()));
        let thread = spawn_render_thread(RING_CAPACITY, host, wake);
        Self {
            thread: Some(thread),
            backlog: VecDeque::new(),
            externals,
            viewport: None,
            stats,
            submitted: 0,
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
    }

    pub fn stats(&self) -> &Arc<RenderStats> {
        &self.stats
    }

    pub fn submitted(&self) -> u64 {
        self.submitted
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
                self.flush();
                true
            }
        }
    }

    pub fn push_diff(&mut self, diff: SceneDiff) {
        if !diff.is_empty() {
            self.backlog.push_back(RenderCommand::Scene(diff));
            self.submitted += 1;
        }
        self.flush();
    }

    pub fn push_commands(&mut self, commands: impl IntoIterator<Item = RenderCommand>) {
        self.backlog.extend(commands);
        self.flush();
    }

    pub fn external(&self, update: ExternalUpdate) {
        let _ = self.externals.send(update);
    }

    pub fn flush(&mut self) {
        let Some(thread) = self.thread.as_mut() else {
            return;
        };
        while let Some(command) = self.backlog.pop_front() {
            if !thread.send(command.clone()) {
                self.backlog.push_front(command);
                break;
            }
        }
    }

    pub fn poll(&mut self) -> Vec<RenderEvent> {
        self.flush();
        match self.thread.as_mut() {
            Some(thread) => thread.poll(),
            None => Vec::new(),
        }
    }

    pub fn wait_event(&mut self, timeout: Duration) -> Option<RenderEvent> {
        self.thread
            .as_mut()
            .and_then(|thread| thread.wait_event(timeout))
    }

    pub fn shutdown(&mut self) -> Option<RenderSide> {
        self.thread.take().and_then(RenderThread::shutdown)
    }
}

impl Drop for RenderLink {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
