use super::client::{failed, Client};
use super::compositor::{Compositor, Rect};
use super::HarnessError;
use rustix::fs::{memfd_create, MemfdFlags};
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::time::Duration;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{Format, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{delegate_noop, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

impl Capture {
    pub fn region(&self, x: u32, y: u32, size: u32) -> Vec<[u8; 4]> {
        (y..y + size)
            .flat_map(|row| (x..x + size).map(move |column| (column, row)))
            .filter_map(|(column, row)| self.bgra_at(column, row))
            .collect()
    }

    pub fn bgra_at(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let offset = usize::try_from(y * self.stride + x * 4).ok()?;
        self.pixels
            .get(offset..offset + 4)
            .and_then(|pixel| pixel.try_into().ok())
    }
}

#[derive(Default)]
struct Copying {
    buffer: Option<(Format, u32, u32, u32)>,
    announced: bool,
    outcome: Option<bool>,
    damage: Vec<Rect>,
}

impl Dispatch<WlRegistry, GlobalListContents> for Copying {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for Copying {
    fn event(
        state: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } if state.buffer.is_none() => state.buffer = Some((format, width, height, stride)),
            zwlr_screencopy_frame_v1::Event::BufferDone => state.announced = true,
            zwlr_screencopy_frame_v1::Event::Damage {
                x,
                y,
                width,
                height,
            } => state.damage.push(Rect {
                x: x as i32,
                y: y as i32,
                width: width as i32,
                height: height as i32,
            }),
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                state.outcome = Some(true);
                frame.destroy();
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.outcome = Some(false);
                frame.destroy();
            }
            _ => {}
        }
    }
}

delegate_noop!(Copying: ZwlrScreencopyManagerV1);
delegate_noop!(Copying: ignore WlOutput);
delegate_noop!(Copying: ignore WlShm);
delegate_noop!(Copying: WlShmPool);
delegate_noop!(Copying: ignore WlBuffer);

struct Bound {
    manager: ZwlrScreencopyManagerV1,
    shm: WlShm,
    output: WlOutput,
}

const NAME: &str = "screencopy";
const LIMIT: Duration = Duration::from_secs(5);

fn connect(compositor: &Compositor) -> Result<(Client<Copying>, Bound), HarnessError> {
    let client = Client::connect(compositor.display(), NAME, Copying::default())?;
    let manager: ZwlrScreencopyManagerV1 = client.bind(3, ())?;
    if manager.version() < 3 {
        return Err(failed(NAME, "screencopy older than version 3"));
    }
    let shm: WlShm = client.bind(1, ())?;
    let output: WlOutput = client.bind(1, ())?;
    Ok((
        client,
        Bound {
            manager,
            shm,
            output,
        },
    ))
}

fn copy(
    client: &mut Client<Copying>,
    bound: &Bound,
    with_cursor: bool,
    with_damage: bool,
    limit: Duration,
) -> Result<Capture, HarnessError> {
    client.state = Copying::default();
    let frame =
        bound
            .manager
            .capture_output(i32::from(with_cursor), &bound.output, &client.handle(), ());
    client.dispatch_until(LIMIT, |state| state.announced)?;
    let (format, width, height, stride) = client
        .state
        .buffer
        .ok_or_else(|| failed(NAME, "the compositor offered no shm buffer"))?;
    let size = stride * height;
    let fd = memfd_create("aqua-test-capture", MemfdFlags::CLOEXEC)
        .map_err(|error| failed(NAME, error))?;
    let file = std::fs::File::from(fd);
    file.set_len(u64::from(size))
        .map_err(|error| failed(NAME, error))?;
    let pool = bound
        .shm
        .create_pool(file.as_fd(), size as i32, &client.handle(), ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        format,
        &client.handle(),
        (),
    );
    match with_damage {
        true => frame.copy_with_damage(&buffer),
        false => frame.copy(&buffer),
    }
    let finished = client.dispatch_until(limit, |state| state.outcome.is_some());
    buffer.destroy();
    pool.destroy();
    finished?;
    if client.state.outcome != Some(true) {
        return Err(failed(NAME, "the compositor failed the copy"));
    }
    let mut pixels = vec![0u8; size as usize];
    file.read_exact_at(&mut pixels, 0)
        .map_err(|error| failed(NAME, error))?;
    Ok(Capture {
        width,
        height,
        stride,
        pixels,
    })
}

pub fn capture(compositor: &Compositor, with_cursor: bool) -> Result<Capture, HarnessError> {
    let (mut client, bound) = connect(compositor)?;
    copy(&mut client, &bound, with_cursor, false, LIMIT)
}

pub struct DamageWatch {
    client: Client<Copying>,
    bound: Bound,
}

impl DamageWatch {
    pub fn start(compositor: &Compositor) -> Result<(DamageWatch, Capture), HarnessError> {
        let (mut client, bound) = connect(compositor)?;
        let first = copy(&mut client, &bound, false, true, LIMIT)?;
        Ok((DamageWatch { client, bound }, first))
    }

    pub fn next(&mut self, limit: Duration) -> Result<(Capture, Vec<Rect>), HarnessError> {
        let capture = copy(&mut self.client, &self.bound, false, true, limit)?;
        Ok((capture, std::mem::take(&mut self.client.state.damage)))
    }
}
