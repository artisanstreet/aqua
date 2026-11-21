use crate::globals::{CLOCK_MONOTONIC, PRESENTATION_CLOCK};
use crate::scale::Scaling;
use aqua_protocol::{Instant, WindowId};
use aqua_shell::ShellError;
use rustix::event::{poll, PollFd, PollFlags};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, WpPresentationFeedback,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceGeometry {
    pub logical: (u32, u32),
    pub physical: (u32, u32),
    pub scale: f64,
    pub scaling: Scaling,
    pub refresh: Option<Duration>,
}

pub(crate) struct SurfaceCore {
    pub conn: Connection,
    pub surface: WlSurface,
    pub viewport: Option<WpViewport>,
    pub fractional: Option<WpFractionalScaleV1>,
    pub presentation: Option<WpPresentation>,
    pub geometry: Mutex<SurfaceGeometry>,
    pub window: WindowId,
    pub commits: AtomicU64,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SurfaceCore {
    pub(crate) fn geometry(&self) -> SurfaceGeometry {
        *locked(&self.geometry)
    }

    pub(crate) fn set_geometry(&self, geometry: SurfaceGeometry) {
        *locked(&self.geometry) = geometry;
    }

    pub(crate) fn commits(&self) -> u64 {
        self.commits.load(Ordering::Acquire)
    }

    pub(crate) fn apply_geometry(&self) -> SurfaceGeometry {
        self.commits.fetch_add(1, Ordering::AcqRel);
        let geometry = self.geometry();
        match (geometry.scaling, &self.viewport) {
            (Scaling::Viewport, Some(viewport)) => viewport.set_destination(
                i32::try_from(geometry.logical.0).unwrap_or(i32::MAX),
                i32::try_from(geometry.logical.1).unwrap_or(i32::MAX),
            ),
            (Scaling::BufferScale(factor), _) if self.surface.version() >= 3 => {
                self.surface.set_buffer_scale(factor)
            }
            _ => {}
        }
        geometry
    }
}

impl Drop for SurfaceCore {
    fn drop(&mut self) {
        if let Some(fractional) = self.fractional.take() {
            fractional.destroy();
        }
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        self.surface.destroy();
        let _ = self.conn.flush();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameWait {
    Ready,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presented {
    Shown {
        at: Option<Instant>,
        refresh: Option<Duration>,
        sequence: u64,
    },
    Discarded,
}

#[derive(Default)]
pub(crate) struct LeaseEvents {
    outstanding: Option<u64>,
    next: u64,
    presented: VecDeque<Presented>,
}

struct Frames {
    queue: EventQueue<LeaseEvents>,
    events: LeaseEvents,
}

pub struct SurfaceLease {
    core: Arc<SurfaceCore>,
    frames: Mutex<Frames>,
}

fn wayland(request: &'static str, reason: impl std::fmt::Display) -> ShellError {
    ShellError::Wayland(request, reason.to_string())
}

impl SurfaceLease {
    pub(crate) fn new(core: Arc<SurfaceCore>) -> Self {
        let queue = core.conn.new_event_queue();
        Self {
            core,
            frames: Mutex::new(Frames {
                queue,
                events: LeaseEvents::default(),
            }),
        }
    }

    pub fn window(&self) -> WindowId {
        self.core.window
    }

    pub fn display_handle(&self) -> u64 {
        self.core.conn.backend().display_ptr() as u64
    }

    pub fn surface_handle(&self) -> u64 {
        self.core.surface.id().as_ptr() as u64
    }

    pub fn geometry(&self) -> SurfaceGeometry {
        self.core.geometry()
    }

    pub fn before_present(&self) -> SurfaceGeometry {
        let mut frames = locked(&self.frames);
        let qh = frames.queue.handle();
        let sequence = frames.events.next;
        frames.events.next += 1;
        frames.events.outstanding = Some(sequence);
        self.core.surface.frame(&qh, sequence);
        if let Some(presentation) = &self.core.presentation {
            presentation.feedback(&self.core.surface, &qh, ());
        }
        self.core.apply_geometry()
    }

    pub fn take_presented(&self) -> Vec<Presented> {
        let mut frames = locked(&self.frames);
        frames.events.presented.drain(..).collect()
    }

    pub fn wait_ready(&self, timeout: Duration) -> Result<FrameWait, ShellError> {
        let deadline = std::time::Instant::now() + timeout;
        let mut frames = locked(&self.frames);
        let Frames { queue, events } = &mut *frames;
        loop {
            queue
                .dispatch_pending(events)
                .map_err(|error| wayland("dispatch", error))?;
            if events.outstanding.is_none() {
                return Ok(FrameWait::Ready);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(FrameWait::TimedOut);
            }
            let _ = self.core.conn.flush();
            let Some(guard) = queue.prepare_read() else {
                continue;
            };
            let readable = {
                let mut fds = [PollFd::from_borrowed_fd(
                    guard.connection_fd(),
                    PollFlags::IN,
                )];
                match poll(&mut fds, Some(&crate::clock::timespec(remaining))) {
                    Ok(_) => fds[0].revents().contains(PollFlags::IN),
                    Err(rustix::io::Errno::INTR) => false,
                    Err(error) => return Err(wayland("poll", error)),
                }
            };
            match readable {
                true => match guard.read() {
                    Ok(_) => {}
                    Err(wayland_client::backend::WaylandError::Io(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(wayland("wl_display_read_events", error)),
                },
                false => drop(guard),
            }
        }
    }
}

impl Dispatch<WlCallback, u64> for LeaseEvents {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        sequence: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event
            && state.outstanding == Some(*sequence)
        {
            state.outstanding = None;
        }
    }
}

impl Dispatch<WpPresentationFeedback, ()> for LeaseEvents {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                ..
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let at = (PRESENTATION_CLOCK.load(Ordering::Acquire) == CLOCK_MONOTONIC)
                    .then(|| crate::clock::from_monotonic(seconds, tv_nsec));
                state.presented.push_back(Presented::Shown {
                    at,
                    refresh: (refresh > 0).then(|| Duration::from_nanos(u64::from(refresh))),
                    sequence: (u64::from(seq_hi) << 32) | u64::from(seq_lo),
                });
            }
            wp_presentation_feedback::Event::Discarded => {
                state.presented.push_back(Presented::Discarded)
            }
            _ => {}
        }
    }
}
