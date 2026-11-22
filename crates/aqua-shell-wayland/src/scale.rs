use crate::state::{Shell, WindowState};
use aqua_protocol::WindowId;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};

pub(crate) const FRACTIONAL_DENOMINATOR: f64 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Scale {
    pub fractional: Option<u32>,
    pub preferred_buffer: Option<i32>,
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scaling {
    Viewport,
    BufferScale(i32),
}

pub(crate) fn physical(logical: (i32, i32), scale: f64) -> (u32, u32) {
    let axis = |value: i32| (f64::from(value.max(1)) * scale).round().max(1.0) as u32;
    (axis(logical.0), axis(logical.1))
}

fn outputs_of<'a>(
    shell: &'a Shell,
    window: &'a WindowState,
) -> impl Iterator<Item = &'a crate::output::Description> + 'a {
    let entered: Vec<_> = shell
        .outputs
        .iter()
        .filter(|output| window.outputs.contains(&output.name))
        .collect();
    let chosen = match entered.is_empty() {
        true => shell.outputs.iter().take(1).collect(),
        false => entered,
    };
    chosen.into_iter().map(|output| &output.current)
}

pub(crate) fn scaling(window: &WindowState) -> Scaling {
    match window.core.viewport.is_some() {
        true => Scaling::Viewport,
        false => Scaling::BufferScale(window.scale.value.round().max(1.0) as i32),
    }
}

pub(crate) fn resolve(shell: &Shell, window: &WindowState) -> f64 {
    let fractional_guess = || {
        outputs_of(shell, window)
            .map(crate::output::Description::fractional_scale)
            .fold(1.0_f64, f64::max)
    };
    let integer_guess = || {
        outputs_of(shell, window)
            .map(|output| output.scale.max(1))
            .max()
            .unwrap_or(1)
    };
    match (window.core.viewport.is_some(), window.scale) {
        (
            true,
            Scale {
                fractional: Some(numerator),
                ..
            },
        ) => f64::from(numerator) / FRACTIONAL_DENOMINATOR,
        (true, _) => fractional_guess(),
        (
            false,
            Scale {
                preferred_buffer: Some(factor),
                ..
            },
        ) => f64::from(factor.max(1)),
        (false, _) => f64::from(integer_guess()),
    }
}

impl Dispatch<WpFractionalScaleV1, WindowId> for Shell {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        id: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            if let Some(window) = state.windows.get_mut(id) {
                window.scale.fractional = Some(scale);
            }
            crate::toplevel::rescale(state, *id);
        }
    }
}
