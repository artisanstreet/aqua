use crate::state::Spec;
use aqua_shell::{Chrome, WindowKind};
use std::sync::Once;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode;

pub(crate) const NATIVE_FALLBACK_BAND: f64 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Frame {
    Server,
    Client { band: Option<f64> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Negotiation {
    Unavailable,
    Granted(Mode),
}

pub(crate) fn requested(spec: &Spec) -> Mode {
    match (spec.kind, spec.chrome) {
        (WindowKind::Normal, Chrome::Native) => Mode::ServerSide,
        _ => Mode::ClientSide,
    }
}

static FALLBACK_LOGGED: Once = Once::new();

pub(crate) fn negotiated(spec: &Spec, negotiation: Negotiation) -> Frame {
    match (spec.kind, spec.chrome, negotiation) {
        (WindowKind::Normal, Chrome::Native, Negotiation::Granted(Mode::ServerSide)) => {
            Frame::Server
        }
        (WindowKind::Normal, Chrome::Native, _) => {
            FALLBACK_LOGGED.call_once(|| {
                eprintln!(
                    "aqua: compositor draws no title bar; Titlebar::Native falls back to a drag band"
                )
            });
            Frame::Client {
                band: Some(NATIVE_FALLBACK_BAND),
            }
        }
        _ => Frame::Client { band: None },
    }
}
