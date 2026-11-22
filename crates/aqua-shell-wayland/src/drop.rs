use crate::data_device::Offer;
use crate::formats::{parse_files, URI_LIST};
use crate::looping::emit;
use crate::state::Shell;
use aqua_core::Point;
use aqua_protocol::WindowId;
use aqua_shell::{FileDrag, ShellEvent};
use std::path::PathBuf;
use std::time::Duration;
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_data_offer::WlDataOffer;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::Proxy;

const READ_LIMIT: Duration = Duration::from_millis(200);

#[derive(Default)]
pub(crate) enum Drag {
    #[default]
    Idle,
    Refused(WlDataOffer),
    Accepted {
        offer: WlDataOffer,
        window: WindowId,
        position: Point,
        paths: Vec<PathBuf>,
    },
}

fn point((x, y): (f64, f64)) -> Point {
    Point::new(x as f32, y as f32)
}

fn read_paths(shell: &Shell, offer: &WlDataOffer) -> Option<Vec<PathBuf>> {
    match crate::clipboard::receive(
        &shell.conn,
        &Offer::Data(offer.clone()),
        URI_LIST,
        READ_LIMIT,
    ) {
        Ok(bytes) => Some(parse_files(&bytes)),
        Err(error) => {
            eprintln!("aqua: file drop: reading the dragged paths failed: {error}");
            None
        }
    }
}

fn dispose(drag: Drag) {
    match drag {
        Drag::Idle => {}
        Drag::Refused(offer) | Drag::Accepted { offer, .. } => offer.destroy(),
    }
}

pub(crate) fn entered(
    shell: &mut Shell,
    serial: u32,
    surface: &WlSurface,
    at: (f64, f64),
    offer: Option<WlDataOffer>,
) {
    dispose(std::mem::take(&mut shell.drag));
    let Some(offer) = offer else {
        return;
    };
    let target = surface
        .data::<WindowId>()
        .copied()
        .filter(|id| shell.windows.get(id).is_some_and(|window| window.file_drop));
    let files = Offer::Data(offer.clone())
        .mimes()
        .iter()
        .any(|mime| mime == URI_LIST);
    let negotiated = offer.version() >= 3;
    shell.drag = match (target, files) {
        (Some(window), true) => {
            offer.accept(serial, Some(URI_LIST.into()));
            if negotiated {
                offer.set_actions(DndAction::Copy, DndAction::Copy);
            }
            let position = point(at);
            let paths = read_paths(shell, &offer).unwrap_or_default();
            emit(
                window,
                ShellEvent::FileDrag(FileDrag::Enter {
                    paths: paths.clone(),
                    position,
                }),
            );
            Drag::Accepted {
                offer,
                window,
                position,
                paths,
            }
        }
        _ => {
            offer.accept(serial, None);
            if negotiated {
                offer.set_actions(DndAction::empty(), DndAction::empty());
            }
            Drag::Refused(offer)
        }
    };
}

pub(crate) fn moved(shell: &mut Shell, at: (f64, f64)) {
    if let Drag::Accepted {
        window, position, ..
    } = &mut shell.drag
    {
        *position = point(at);
        emit(
            *window,
            ShellEvent::FileDrag(FileDrag::Over {
                position: *position,
            }),
        );
    }
}

pub(crate) fn left(shell: &mut Shell) {
    let drag = std::mem::take(&mut shell.drag);
    if let Drag::Accepted { window, .. } = &drag {
        emit(*window, ShellEvent::FileDrag(FileDrag::Leave));
    }
    dispose(drag);
}

pub(crate) fn dropped(shell: &mut Shell) {
    let drag = std::mem::take(&mut shell.drag);
    let Drag::Accepted {
        offer,
        window,
        position,
        paths,
    } = drag
    else {
        dispose(drag);
        return;
    };
    let paths = read_paths(shell, &offer)
        .filter(|fresh| !fresh.is_empty())
        .unwrap_or(paths);
    emit(
        window,
        ShellEvent::FileDrag(FileDrag::Drop { paths, position }),
    );
    if offer.version() >= 3 {
        offer.finish();
    }
    offer.destroy();
}
