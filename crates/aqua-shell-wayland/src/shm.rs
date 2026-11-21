use crate::state::Shell;
use aqua_protocol::WindowId;
use rustix::fs::{memfd_create, MemfdFlags};
use std::io::Write;
use std::os::fd::AsFd;
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_shm::Format;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};

pub(crate) fn present(
    shell: &mut Shell,
    id: WindowId,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> bool {
    let expected = (width as usize) * (height as usize) * 4;
    let (Ok(stride), Ok(size)) = (i32::try_from(width * 4), i32::try_from(expected)) else {
        return false;
    };
    if width == 0 || height == 0 || pixels.len() < expected {
        return false;
    }
    let (Some(shm), Some(window)) = (&shell.globals.shm, shell.windows.get(&id)) else {
        return false;
    };
    let Ok(fd) = memfd_create("aqua-present", MemfdFlags::CLOEXEC) else {
        return false;
    };
    let mut file = std::fs::File::from(fd);
    if file.write_all(&pixels[..expected]).is_err() {
        return false;
    }
    let format = match window.spec.alpha_surface {
        true => Format::Argb8888,
        false => Format::Xrgb8888,
    };
    let pool = shm.create_pool(file.as_fd(), size, &shell.qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride,
        format,
        &shell.qh,
        (),
    );
    pool.destroy();
    let surface = &window.core.surface;
    surface.attach(Some(&buffer), 0, 0);
    window.core.apply_geometry((width, height));
    surface.damage_buffer(0, 0, width as i32, height as i32);
    surface.commit();
    true
}

impl Dispatch<WlBuffer, ()> for Shell {
    fn event(
        _: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

delegate_noop!(Shell: WlShmPool);
