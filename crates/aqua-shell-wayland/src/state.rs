use crate::decoration::Frame;
use crate::globals::Globals;
use crate::input::Seat;
use crate::lease::SurfaceCore;
use crate::output::Output;
use crate::scale::Scale;
use crate::toplevel::{Role, States};
use aqua_core::CursorIcon;
use aqua_protocol::chrome::DragRects;
use aqua_protocol::{DragRegions, WindowId};
use aqua_shell::{Chrome, ShellError, WindowKind};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use wayland_client::{Connection, EventQueue, QueueHandle};

pub(crate) struct Spec {
    pub title: String,
    pub app_id: String,
    pub chrome: Chrome,
    pub kind: WindowKind,
    pub min_logical: Option<(i32, i32)>,
    pub resizable: bool,
    pub minimizable: bool,
    pub maximizable: bool,
    pub alpha_surface: bool,
    pub owner: Option<WindowId>,
}

pub(crate) struct WindowState {
    pub id: WindowId,
    pub core: Arc<SurfaceCore>,
    pub role: Role,
    pub spec: Spec,
    pub logical: (i32, i32),
    pub restore: (i32, i32),
    pub scale: Scale,
    pub states: States,
    pub frame: Frame,
    pub outputs: Vec<u32>,
    pub regions: DragRegions,
    pub rects: DragRects,
    pub cursor: CursorIcon,
    pub redraw_pending: bool,
    pub covered: bool,
    pub starved: bool,
    pub probe: crate::occlusion::Probe,
    pub announced: Option<Announced>,
    pub file_drop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Announced {
    pub scale: f64,
    pub physical: (u32, u32),
}

impl WindowState {
    pub(crate) fn physical(&self) -> (u32, u32) {
        crate::scale::physical(self.logical, self.scale.value)
    }
}

pub(crate) struct Shell {
    pub conn: Connection,
    pub qh: QueueHandle<Shell>,
    pub globals: Globals,
    pub outputs: Vec<Output>,
    pub windows: HashMap<WindowId, WindowState>,
    pub seat: Seat,
    pub transfer: crate::data_device::Transfer,
    pub drag: crate::drop::Drag,
    pub text_input: crate::text_input::Transport,
    pub external: crate::external::External,
    pub next_window: u64,
}

pub(crate) enum Endpoint {
    Environment,
    Socket { path: PathBuf, hidden: Vec<String> },
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
    static QUEUE: RefCell<Option<EventQueue<Shell>>> = const { RefCell::new(None) };
}

fn wayland(request: &'static str, reason: impl std::fmt::Display) -> ShellError {
    ShellError::Wayland(request, reason.to_string())
}

pub(crate) fn connect(endpoint: Endpoint) -> Result<(), ShellError> {
    let (conn, hidden) = match endpoint {
        Endpoint::Environment => (
            Connection::connect_to_env().map_err(|error| wayland("wl_display_connect", error))?,
            Vec::new(),
        ),
        Endpoint::Socket { path, hidden } => {
            let stream = std::os::unix::net::UnixStream::connect(&path)
                .map_err(|error| wayland("wl_display_connect", error))?;
            (
                Connection::from_socket(stream)
                    .map_err(|error| wayland("wl_display_connect", error))?,
                hidden,
            )
        }
    };
    let mut queue = conn.new_event_queue::<Shell>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut shell = Shell {
        conn: conn.clone(),
        qh,
        globals: Globals::new(hidden),
        outputs: Vec::new(),
        windows: HashMap::new(),
        seat: Seat::default(),
        transfer: crate::data_device::Transfer::default(),
        drag: crate::drop::Drag::default(),
        text_input: crate::text_input::Transport::default(),
        external: crate::external::External::default(),
        next_window: 1,
    };
    for _ in 0..2 {
        queue
            .roundtrip(&mut shell)
            .map_err(|error| wayland("wl_display.sync", error))?;
    }
    if let Some(missing) = shell.globals.missing() {
        return Err(wayland(
            "wl_registry",
            format!("the compositor has no {missing}"),
        ));
    }
    disconnect();
    SHELL.with_borrow_mut(|slot| *slot = Some(shell));
    QUEUE.with_borrow_mut(|slot| *slot = Some(queue));
    Ok(())
}

pub(crate) fn disconnect() {
    let shell = SHELL.with_borrow_mut(Option::take);
    let queue = QUEUE.with_borrow_mut(Option::take);
    drop(shell);
    drop(queue);
}

pub(crate) fn ensure() -> Result<(), ShellError> {
    match SHELL.with_borrow(Option::is_some) {
        true => Ok(()),
        false => connect(Endpoint::Environment),
    }
}

pub(crate) fn connected() -> bool {
    SHELL.with_borrow(Option::is_some)
}

pub(crate) fn with<R>(f: impl FnOnce(&mut Shell) -> R) -> R {
    SHELL.with_borrow_mut(|shell| {
        f(shell.as_mut().expect(
            "the Wayland shell is not connected on this thread; run or Window::create connects it",
        ))
    })
}

pub(crate) fn try_with<R>(f: impl FnOnce(&mut Shell) -> R) -> Option<R> {
    SHELL.with_borrow_mut(|shell| shell.as_mut().map(f))
}

pub(crate) fn window<R>(id: WindowId, f: impl FnOnce(&mut WindowState) -> R) -> Option<R> {
    try_with(|shell| shell.windows.get_mut(&id).map(f)).flatten()
}

pub(crate) fn dispatch_pending() -> Result<usize, ShellError> {
    QUEUE.with_borrow_mut(|queue| {
        let queue = queue
            .as_mut()
            .expect("the Wayland event queue lives as long as the shell");
        SHELL.with_borrow_mut(|shell| {
            queue
                .dispatch_pending(shell.as_mut().expect("the shell outlives its queue"))
                .map_err(|error| wayland("dispatch", error))
        })
    })
}

pub(crate) fn prepare_read() -> Option<wayland_client::backend::ReadEventsGuard> {
    QUEUE.with_borrow(|queue| queue.as_ref().and_then(EventQueue::prepare_read))
}

pub(crate) fn flush() -> Result<(), ShellError> {
    match try_with(|shell| shell.conn.flush()) {
        None | Some(Ok(())) => Ok(()),
        Some(Err(wayland_client::backend::WaylandError::Io(error)))
            if error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            Ok(())
        }
        Some(Err(error)) => Err(wayland("wl_display_flush", error)),
    }
}

impl Drop for WindowState {
    fn drop(&mut self) {
        self.role = Role::Unmapped;
    }
}
