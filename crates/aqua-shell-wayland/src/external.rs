use std::collections::HashMap;
use std::time::Duration;

use aqua_protocol::WindowId;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{
    self, XdgActivationTokenV1,
};
use wayland_protocols::xdg::foreign::zv1::client::zxdg_exported_v1::{self, ZxdgExportedV1};
use wayland_protocols::xdg::foreign::zv1::client::zxdg_exporter_v1::ZxdgExporterV1;
use wayland_protocols::xdg::foreign::zv2::client::zxdg_exported_v2::{self, ZxdgExportedV2};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_exporter_v2::ZxdgExporterV2;

use crate::state::{self, Shell};
use crate::unsupported::Unsupported;

const ANSWER_LIMIT: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
pub(crate) enum Exporter {
    V2(ZxdgExporterV2),
    V1(ZxdgExporterV1),
}

#[derive(Debug)]
enum Exported {
    V2(ZxdgExportedV2),
    V1(ZxdgExportedV1),
}

impl Exported {
    fn destroy(&self) {
        match self {
            Exported::V2(exported) => exported.destroy(),
            Exported::V1(exported) => exported.destroy(),
        }
    }
}

#[derive(Debug)]
enum Export {
    Requested(Exported),
    Ready { exported: Exported, handle: String },
}

impl Export {
    fn handle(&self) -> Option<&str> {
        match self {
            Export::Requested(_) => None,
            Export::Ready { handle, .. } => Some(handle),
        }
    }

    fn exported(&self) -> &Exported {
        match self {
            Export::Requested(exported) | Export::Ready { exported, .. } => exported,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Purpose {
    Activate(WindowId),
    Handout(u64),
}

#[derive(Debug, Default)]
pub(crate) struct External {
    exports: HashMap<WindowId, Export>,
    handouts: HashMap<u64, Option<String>>,
    next_handout: u64,
}

impl External {
    pub(crate) fn forget(&mut self, window: WindowId) {
        if let Some(export) = self.exports.remove(&window) {
            export.exported().destroy();
        }
    }
}

pub(crate) fn exported_handle(window: WindowId) -> Result<String, Unsupported> {
    let requested = state::try_with(|shell| {
        if let Some(handle) = shell.external.exports.get(&window).and_then(Export::handle) {
            return Ok(Some(handle.to_owned()));
        }
        if shell.external.exports.contains_key(&window) {
            return Ok(None);
        }
        let exporter = shell.globals.exporter.clone().ok_or(Unsupported::Export)?;
        let surface = shell
            .windows
            .get(&window)
            .filter(|record| record.role.toplevel().is_some())
            .map(|record| record.core.surface.clone())
            .ok_or(Unsupported::Export)?;
        let exported = match exporter {
            Exporter::V2(exporter) => {
                Exported::V2(exporter.export_toplevel(&surface, &shell.qh, window))
            }
            Exporter::V1(exporter) => Exported::V1(exporter.export(&surface, &shell.qh, window)),
        };
        shell
            .external
            .exports
            .insert(window, Export::Requested(exported));
        Ok(None)
    })
    .unwrap_or(Err(Unsupported::Export))?;
    if let Some(handle) = requested {
        return Ok(handle);
    }
    let answered = crate::looping::wait_until(ANSWER_LIMIT, "xdg_exported.handle", || {
        state::try_with(|shell| {
            shell
                .external
                .exports
                .get(&window)
                .is_some_and(|export| export.handle().is_some())
        })
    });
    match answered {
        Ok(()) => state::try_with(|shell| {
            shell
                .external
                .exports
                .get(&window)
                .and_then(Export::handle)
                .map(str::to_owned)
        })
        .flatten()
        .ok_or(Unsupported::Export),
        Err(_) => Err(Unsupported::Export),
    }
}

fn request_token(shell: &mut Shell, window: WindowId, purpose: Purpose) -> Result<(), Unsupported> {
    let activation = shell
        .globals
        .activation
        .clone()
        .ok_or(Unsupported::Activation)?;
    let surface = shell
        .windows
        .get(&window)
        .map(|record| record.core.surface.clone())
        .ok_or(Unsupported::Activation)?;
    let token = activation.get_activation_token(&shell.qh, purpose);
    if let (Some(serial), Some((_, seat))) = (shell.seat.serial, &shell.globals.seat) {
        token.set_serial(serial, seat);
    }
    token.set_surface(&surface);
    token.commit();
    Ok(())
}

pub(crate) fn activate(window: WindowId) -> Result<(), Unsupported> {
    state::try_with(|shell| request_token(shell, window, Purpose::Activate(window)))
        .unwrap_or(Err(Unsupported::Activation))
}

pub(crate) fn activation_token(window: WindowId) -> Result<String, Unsupported> {
    let handout = state::try_with(|shell| {
        let handout = shell.external.next_handout;
        shell.external.next_handout += 1;
        request_token(shell, window, Purpose::Handout(handout))?;
        shell.external.handouts.insert(handout, None);
        Ok(handout)
    })
    .unwrap_or(Err(Unsupported::Activation))?;
    let answered = crate::looping::wait_until(ANSWER_LIMIT, "xdg_activation_token.done", || {
        state::try_with(|shell| {
            shell
                .external
                .handouts
                .get(&handout)
                .is_some_and(Option::is_some)
        })
    });
    let token = state::try_with(|shell| shell.external.handouts.remove(&handout))
        .flatten()
        .flatten();
    match (answered, token) {
        (Ok(()), Some(token)) => Ok(token),
        _ => Err(Unsupported::Activation),
    }
}

pub(crate) fn activate_with_token(window: WindowId, token: &str) -> Result<(), Unsupported> {
    state::try_with(|shell| {
        let activation = shell
            .globals
            .activation
            .as_ref()
            .ok_or(Unsupported::Activation)?;
        let surface = shell
            .windows
            .get(&window)
            .map(|record| &record.core.surface)
            .ok_or(Unsupported::Activation)?;
        activation.activate(token.to_owned(), surface);
        Ok(())
    })
    .unwrap_or(Err(Unsupported::Activation))
}

impl Dispatch<XdgActivationTokenV1, Purpose> for Shell {
    fn event(
        state: &mut Self,
        token: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        purpose: &Purpose,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_activation_token_v1::Event::Done { token: value } = event else {
            return;
        };
        match *purpose {
            Purpose::Activate(window) => {
                if let (Some(activation), Some(record)) =
                    (&state.globals.activation, state.windows.get(&window))
                {
                    activation.activate(value, &record.core.surface);
                }
            }
            Purpose::Handout(handout) => {
                if let Some(slot) = state.external.handouts.get_mut(&handout) {
                    *slot = Some(value);
                }
            }
        }
        token.destroy();
    }
}

fn ready(state: &mut Shell, window: WindowId, handle: String) {
    if let Some(export) = state.external.exports.remove(&window) {
        let exported = match export {
            Export::Requested(exported) | Export::Ready { exported, .. } => exported,
        };
        state
            .external
            .exports
            .insert(window, Export::Ready { exported, handle });
    }
}

impl Dispatch<ZxdgExportedV2, WindowId> for Shell {
    fn event(
        state: &mut Self,
        _: &ZxdgExportedV2,
        event: zxdg_exported_v2::Event,
        window: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_exported_v2::Event::Handle { handle } = event {
            ready(state, *window, handle);
        }
    }
}

impl Dispatch<ZxdgExportedV1, WindowId> for Shell {
    fn event(
        state: &mut Self,
        _: &ZxdgExportedV1,
        event: zxdg_exported_v1::Event,
        window: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_exported_v1::Event::Handle { handle } = event {
            ready(state, *window, handle);
        }
    }
}

wayland_client::delegate_noop!(Shell: ZxdgExporterV2);
wayland_client::delegate_noop!(Shell: ZxdgExporterV1);

pub(crate) fn proxy_pointers(window: WindowId) -> Option<(usize, usize)> {
    state::try_with(|shell| {
        let surface = shell.windows.get(&window)?.core.surface.id().as_ptr() as usize;
        let display = shell.conn.backend().display_ptr() as usize;
        Some((display, surface))
    })
    .flatten()
}
