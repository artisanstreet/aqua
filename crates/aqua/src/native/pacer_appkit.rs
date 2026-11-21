use std::time::Duration;

use aqua_gpu_metal::window::pacing::SurfacePacer;
use aqua_shell_appkit::{from_media_time, LayerLease};

pub struct LayerPacer(pub LayerLease);

impl SurfacePacer for LayerPacer {
    fn refresh(&self) -> Option<Duration> {
        self.0.geometry().refresh
    }

    fn covered(&self) -> bool {
        self.0.geometry().covered
    }

    fn presented_at(&self, media_seconds: f64) -> aqua_protocol::Instant {
        from_media_time(media_seconds)
    }
}
