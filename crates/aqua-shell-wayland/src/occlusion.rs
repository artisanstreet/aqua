use crate::looping::emit;
use crate::state::{Shell, WindowState};
use aqua_protocol::{Instant, WindowId};
use aqua_shell::ShellEvent;
use std::time::Duration;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::{Connection, Dispatch, QueueHandle};

pub(crate) const STARVED: Duration = Duration::from_millis(500);
const COMMITS_BEFORE_TIMING: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Probe {
    #[default]
    Idle,
    Requested {
        commits: u64,
    },
    Starving {
        since: Instant,
    },
}

pub(crate) fn request(shell: &mut Shell, id: WindowId) {
    let qh = shell.qh.clone();
    if let Some(window) = shell.windows.get_mut(&id)
        && window.probe == Probe::Idle
        && window.role.configured()
    {
        window.core.surface.frame(&qh, id);
        window.probe = Probe::Requested {
            commits: window.core.commits(),
        };
    }
}

pub(crate) fn settle(window: &mut WindowState) {
    let covered = window.role.configured() && (window.states.suspended || window.starved);
    if window.covered != covered {
        window.covered = covered;
        emit(window.id, ShellEvent::Covered(covered));
    }
}

pub(crate) fn step(shell: &mut Shell, now: Instant) {
    let ids: Vec<WindowId> = shell.windows.keys().copied().collect();
    for id in ids {
        request(shell, id);
        let Some(window) = shell.windows.get_mut(&id) else {
            continue;
        };
        window.probe = match window.probe {
            Probe::Requested { commits }
                if window.core.commits() >= commits + COMMITS_BEFORE_TIMING =>
            {
                Probe::Starving { since: now }
            }
            Probe::Starving { since } if now.saturating_duration_since(since) >= STARVED => {
                window.starved = true;
                Probe::Starving { since }
            }
            probe => probe,
        };
        settle(window);
    }
}

pub(crate) fn deadline(shell: &Shell) -> Option<Instant> {
    shell
        .windows
        .values()
        .filter(|window| !window.starved)
        .filter_map(|window| match window.probe {
            Probe::Starving { since } => Some(since + STARVED),
            Probe::Requested { commits }
                if window.core.commits() >= commits + COMMITS_BEFORE_TIMING =>
            {
                Some(crate::clock::now())
            }
            _ => None,
        })
        .min()
}

impl Dispatch<WlCallback, WindowId> for Shell {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        id: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event
            && let Some(window) = state.windows.get_mut(id)
        {
            window.probe = Probe::Idle;
            window.starved = false;
            settle(window);
        }
    }
}
