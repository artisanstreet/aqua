use std::time::Duration;

use aqua_gpu_vulkan::window::pacing::{FrameWait, PacerError, Presentation, SurfacePacer};
use aqua_graph::Extent;
use aqua_shell_wayland::{Presented, SurfaceLease};

pub struct LeasePacer(pub SurfaceLease);

impl SurfacePacer for LeasePacer {
    fn before_present(&mut self, buffer: Extent) {
        self.0.before_present((buffer.width, buffer.height));
    }

    fn wait_ready(&mut self, timeout: Duration) -> Result<FrameWait, PacerError> {
        match self.0.wait_ready(timeout) {
            Ok(aqua_shell_wayland::FrameWait::Ready) => Ok(FrameWait::Ready),
            Ok(aqua_shell_wayland::FrameWait::TimedOut) => Ok(FrameWait::TimedOut),
            Err(error) => Err(PacerError {
                reason: error.to_string(),
            }),
        }
    }

    fn take_presented(&mut self) -> Vec<Presentation> {
        self.0
            .take_presented()
            .into_iter()
            .map(|presented| match presented {
                Presented::Shown { at, refresh, .. } => Presentation::Shown { at, refresh },
                Presented::Discarded => Presentation::Discarded,
            })
            .collect()
    }

    fn refresh(&self) -> Option<Duration> {
        self.0.geometry().refresh
    }
}
