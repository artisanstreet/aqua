use std::net::{Ipv4Addr, SocketAddr};

use aqua_a11y_uia::{inspect, render_json};
use aqua_protocol::AccessTreeUpdate;
use aqua_transport::{FrameRecord, Inspector, InspectorServer};

use crate::render::host::RenderStats;

pub const ENV: &str = "AQUA_INSPECTOR";

pub struct HostInspector {
    inspector: Inspector,
    _server: InspectorServer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortSpec {
    Ephemeral,
    Fixed(u16),
}

pub fn parse_port(value: &str) -> Option<PortSpec> {
    match value.trim() {
        "" => None,
        "0" | "auto" | "1" | "true" => Some(PortSpec::Ephemeral),
        other => other.parse().ok().map(PortSpec::Fixed),
    }
}

impl HostInspector {
    pub fn from_env() -> Option<Self> {
        let spec = parse_port(&std::env::var(ENV).ok()?)?;
        let port = match spec {
            PortSpec::Ephemeral => 0,
            PortSpec::Fixed(port) => port,
        };
        let inspector = Inspector::new();
        match InspectorServer::bind(
            inspector.clone(),
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        ) {
            Ok(server) => {
                eprintln!("aqua: inspector listening on {}", server.local_addr());
                Some(Self {
                    inspector,
                    _server: server,
                })
            }
            Err(error) => {
                eprintln!("aqua: inspector unavailable: {error}");
                None
            }
        }
    }

    pub fn handle(&self) -> Inspector {
        self.inspector.clone()
    }

    pub fn publish_stats(&self, stats: &RenderStats) {
        use std::sync::atomic::Ordering::Relaxed;
        let frames = stats.frames.load(Relaxed);
        let failures = stats.failures.load(Relaxed);
        let recoveries = stats.recoveries.load(Relaxed);
        let current = self.inspector.snapshot();
        let unchanged = current.frames_presented == frames
            && current.frames_failed == failures
            && u64::from(current.device_losses) == recoveries;
        if unchanged {
            return;
        }
        let per_frame = |nanos: u64| nanos / 1000 / frames.max(1);
        let record = FrameRecord {
            frame: frames,
            commands: u32::try_from(stats.commands.load(Relaxed)).unwrap_or(u32::MAX),
            unsupported_items: u32::try_from(stats.skipped_items.load(Relaxed)).unwrap_or(u32::MAX),
            cpu_micros: per_frame(stats.execute_ns.load(Relaxed)),
            present_queued_micros: per_frame(stats.present_ns.load(Relaxed)),
            ..FrameRecord::default()
        };
        self.inspector.update(|snapshot| {
            snapshot.frames_presented = frames;
            snapshot.frames_failed = failures;
            snapshot.device_losses = u32::try_from(recoveries).unwrap_or(u32::MAX);
            snapshot.last = Some(record);
        });
    }
}

pub fn publish_tree(inspector: &Inspector, update: &AccessTreeUpdate) {
    let json = inspect(update).map(|root| render_json(&root));
    if inspector.snapshot().tree_json == json {
        return;
    }
    inspector.update(|snapshot| snapshot.tree_json = json);
}
