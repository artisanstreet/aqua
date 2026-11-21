use crate::state::Shell;
use aqua_protocol::WindowId;
use std::time::Duration;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};

#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub bounds: (i32, i32, i32, i32),
    pub work_area: (i32, i32, i32, i32),
    pub scale: f64,
    pub refresh: Option<Duration>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Description {
    pub name: String,
    pub mode: (i32, i32),
    pub refresh_mhz: i32,
    pub scale: i32,
    pub logical_position: Option<(i32, i32)>,
    pub logical_size: Option<(i32, i32)>,
}

impl Description {
    pub(crate) fn fractional_scale(&self) -> f64 {
        match self.logical_size {
            Some((width, _)) if width > 0 && self.mode.0 > 0 => {
                f64::from(self.mode.0) / f64::from(width)
            }
            _ => f64::from(self.scale.max(1)),
        }
    }

    pub(crate) fn refresh(&self) -> Option<Duration> {
        u64::try_from(self.refresh_mhz)
            .ok()
            .filter(|mhz| *mhz > 0)
            .map(|mhz| Duration::from_nanos(1_000_000_000_000 / mhz))
    }

    fn monitor(&self) -> Monitor {
        let scale = self.fractional_scale();
        let (x, y) = self.logical_position.unwrap_or((0, 0));
        let left = (f64::from(x) * scale).round() as i32;
        let top = (f64::from(y) * scale).round() as i32;
        let bounds = (left, top, left + self.mode.0, top + self.mode.1);
        Monitor {
            name: self.name.clone(),
            bounds,
            work_area: bounds,
            scale,
            refresh: self.refresh(),
        }
    }
}

pub(crate) struct Output {
    pub name: u32,
    pub proxy: WlOutput,
    pub xdg: Option<ZxdgOutputV1>,
    pub current: Description,
    pending: Description,
}

impl Output {
    pub(crate) fn new(name: u32, proxy: WlOutput) -> Self {
        Self {
            name,
            proxy,
            xdg: None,
            current: Description::default(),
            pending: Description {
                scale: 1,
                ..Description::default()
            },
        }
    }

    pub(crate) fn describe(&mut self, manager: &ZxdgOutputManagerV1, qh: &QueueHandle<Shell>) {
        if self.xdg.is_none() {
            self.xdg = Some(manager.get_xdg_output(&self.proxy, qh, self.name));
        }
    }

    fn commit(&mut self) {
        self.current = self.pending.clone();
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        if let Some(xdg) = self.xdg.take() {
            xdg.destroy();
        }
        if self.proxy.version() >= 3 {
            self.proxy.release();
        }
    }
}

pub(crate) fn monitor_of(shell: &Shell, entered: &[u32]) -> Option<Monitor> {
    entered
        .iter()
        .find_map(|name| shell.outputs.iter().find(|output| output.name == *name))
        .or_else(|| shell.outputs.first())
        .map(|output| output.current.monitor())
}

pub(crate) fn primary_refresh(shell: &Shell, entered: &[u32]) -> Option<Duration> {
    monitor_of(shell, entered).and_then(|monitor| monitor.refresh)
}

pub(crate) fn entered(shell: &mut Shell, id: WindowId, output: &WlOutput) {
    let Some(name) = shell
        .outputs
        .iter()
        .find(|candidate| candidate.proxy == *output)
        .map(|candidate| candidate.name)
    else {
        return;
    };
    let added =
        shell
            .windows
            .get_mut(&id)
            .is_some_and(|window| match window.outputs.contains(&name) {
                true => false,
                false => {
                    window.outputs.push(name);
                    true
                }
            });
    if added {
        crate::toplevel::outputs_changed(shell, id);
    }
}

pub(crate) fn left(shell: &mut Shell, id: WindowId, name: u32) {
    let removed = shell.windows.get_mut(&id).is_some_and(|window| {
        let before = window.outputs.len();
        window.outputs.retain(|entered| *entered != name);
        window.outputs.len() != before
    });
    if removed {
        crate::toplevel::outputs_changed(shell, id);
    }
}

pub(crate) fn left_proxy(shell: &mut Shell, id: WindowId, output: &WlOutput) {
    if let Some(name) = shell
        .outputs
        .iter()
        .find(|candidate| candidate.proxy == *output)
        .map(|candidate| candidate.name)
    {
        left(shell, id, name);
    }
}

fn output_mut(shell: &mut Shell, name: u32) -> Option<&mut Output> {
    shell.outputs.iter_mut().find(|output| output.name == name)
}

fn described(shell: &mut Shell, name: u32) {
    let changed = output_mut(shell, name).is_some_and(|output| {
        let before = output.current.clone();
        output.commit();
        before != output.current
    });
    if changed {
        let ids: Vec<WindowId> = shell.windows.keys().copied().collect();
        for id in ids {
            crate::toplevel::outputs_changed(shell, id);
        }
    }
}

impl Dispatch<WlOutput, u32> for Shell {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let name = *name;
        match event {
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                refresh,
            } if flags.contains(wl_output::Mode::Current) => {
                if let Some(output) = output_mut(state, name) {
                    output.pending.mode = (width, height);
                    output.pending.refresh_mhz = refresh;
                }
            }
            wl_output::Event::Scale { factor } => {
                if let Some(output) = output_mut(state, name) {
                    output.pending.scale = factor;
                }
            }
            wl_output::Event::Name { name: label } => {
                if let Some(output) = output_mut(state, name) {
                    output.pending.name = label;
                }
            }
            wl_output::Event::Done => described(state, name),
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, u32> for Shell {
    fn event(
        state: &mut Self,
        xdg: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let name = *name;
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                if let Some(output) = output_mut(state, name) {
                    output.pending.logical_position = Some((x, y));
                }
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                if let Some(output) = output_mut(state, name) {
                    output.pending.logical_size = Some((width, height));
                }
            }
            zxdg_output_v1::Event::Done if xdg.version() < 3 => described(state, name),
            _ => {}
        }
    }
}
