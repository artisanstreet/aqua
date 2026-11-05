use std::thread::sleep;
use std::time::Duration;

use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_gpu_d3d11::hal::selection::AdapterPreference;
use aqua_gpu_d3d11::window::{SurfaceKind, WindowBackend};
use aqua_gpu_ref::RefBackend;
use aqua_graph::{Extent, FramePlan, PaintCommand, Quad, plan_paint};
use aqua_protocol::{
    FrameInputs, GpuBackend, GpuReadback, PresentHint, SurfaceId, SurfaceTarget, WindowMaterial,
};
use aqua_shell_win32::{Window, WindowOptions};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDIBits, HGDIOBJ, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetWindowRect, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};

use crate::backend::options;

const SOLID: [u8; 3] = [0x3b, 0x82, 0xf6];

fn quad(bounds: Rect, hex: &str, alpha: f32) -> PaintCommand {
    PaintCommand::Quad(Quad {
        bounds,
        radius: 0.0,
        border_width: 0.0,
        fill: Oklch::from_hex(hex).unwrap().with_alpha(alpha),
        border_color: Oklch::from_hex("#000000").unwrap(),
    })
}

fn translucent_scene(extent: Extent) -> FramePlan {
    let commands = [
        quad(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 120.0,
                h: 80.0,
            },
            "#3b82f6",
            1.0,
        ),
        quad(
            Rect {
                x: 140.0,
                y: 0.0,
                w: 120.0,
                h: 80.0,
            },
            "#3b82f6",
            0.5,
        ),
    ];
    plan_paint(
        extent,
        Oklch::from_hex("#000000").unwrap().with_alpha(0.0),
        &commands,
    )
    .unwrap()
}

fn opaque_black_scene(extent: Extent) -> FramePlan {
    plan_paint(extent, Oklch::from_hex("#000000").unwrap(), &[]).unwrap()
}

fn pump() {
    let mut message = MSG::default();
    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn settle() {
    for _ in 0..10 {
        pump();
        sleep(Duration::from_millis(30));
    }
}

struct Capture {
    width: usize,
    bgra: Vec<u8>,
}

impl Capture {
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let at = (y * self.width + x) * 4;
        [
            self.bgra[at + 2],
            self.bgra[at + 1],
            self.bgra[at],
            self.bgra[at + 3],
        ]
    }
}

fn capture_client(window: Window, client: Extent) -> Option<Capture> {
    let hwnd = HWND(window.raw_hwnd() as *mut _);
    let mut outer = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut outer) }.ok()?;
    let mut origin = POINT { x: 0, y: 0 };
    if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
        return None;
    }
    let (offset_x, offset_y) = (
        (origin.x - outer.left) as usize,
        (origin.y - outer.top) as usize,
    );
    let (width, height) = (
        (outer.right - outer.left) as usize,
        (outer.bottom - outer.top) as usize,
    );
    let dc = unsafe { CreateCompatibleDC(None) };
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap =
        unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
    let previous = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
    let printed = unsafe { PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(2)) }.as_bool();
    let mut full = vec![0u8; width * height * 4];
    let copied = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(full.as_mut_ptr().cast()),
            &info as *const _ as *mut _,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        SelectObject(dc, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
    }
    if !printed || copied == 0 {
        return None;
    }
    let mut bgra = Vec::with_capacity(client.width as usize * client.height as usize * 4);
    for y in 0..client.height as usize {
        let start = ((y + offset_y) * width + offset_x) * 4;
        bgra.extend_from_slice(&full[start..start + client.width as usize * 4]);
    }
    Some(Capture {
        width: client.width as usize,
        bgra,
    })
}

fn open_composed(
    backend: &mut WindowBackend,
    material: WindowMaterial,
) -> (Window, SurfaceId, Extent) {
    let window = Window::create(&WindowOptions {
        title: "aqua-composition-test".into(),
        logical_size: (480.0, 300.0),
        alpha_surface: true,
        material,
        dark: true,
        visible: false,
        ..Default::default()
    })
    .expect("alpha window");
    assert!(window.has_alpha_surface());
    let (width, height) = window.inner_size();
    let extent = Extent { width, height };
    let id = backend
        .create_composition_surface(window.id(), extent)
        .expect("composition surface");
    assert_eq!(backend.surface_kind(id), Some(SurfaceKind::Composition));
    let content = backend.composition_content(id).expect("content");
    window.attach_composition_surface(&content).expect("attach");
    window.show_inactive();
    window.send_to_back();
    (window, id, extent)
}

fn render(backend: &mut WindowBackend, id: SurfaceId, plan: &FramePlan) {
    backend.wait_for_frame_slot(id).unwrap();
    backend.execute(plan, &FrameInputs::new(id)).unwrap();
    backend.present(id, PresentHint::Vsync).unwrap();
}

fn composes(preference: AdapterPreference, material: WindowMaterial, label: &str) {
    crate::support::hold_software();
    let mut backend = WindowBackend::new(options(preference))
        .unwrap_or_else(|error| panic!("no adapter {preference:?}: {error}"));
    let (window, id, extent) = open_composed(&mut backend, material);
    let plan = translucent_scene(extent);

    backend.wait_for_frame_slot(id).unwrap();
    backend.execute(&plan, &FrameInputs::new(id)).unwrap();
    let raw = backend.read_surface(id).unwrap();
    backend.present(id, PresentHint::Vsync).unwrap();
    let at = |x: usize, y: usize| {
        let index = (y * extent.width as usize + x) * 4;
        [
            raw.rgba8[index],
            raw.rgba8[index + 1],
            raw.rgba8[index + 2],
            raw.rgba8[index + 3],
        ]
    };
    let mut reference = RefBackend::new();
    let reference_id = reference
        .create_surface(SurfaceTarget::Offscreen(extent))
        .unwrap();
    reference
        .execute(&plan, &FrameInputs::new(reference_id))
        .unwrap();
    let expected = reference.read_surface(reference_id).unwrap();
    let expected_at = |x: usize, y: usize| {
        let index = (y * extent.width as usize + x) * 4;
        [
            expected.rgba8[index],
            expected.rgba8[index + 1],
            expected.rgba8[index + 2],
            expected.rgba8[index + 3],
        ]
    };
    assert_eq!(
        at(300, 200),
        [0, 0, 0, 0],
        "[{label}] transparent pixels must be premultiplied zero"
    );
    let opaque = at(40, 40);
    let half = at(200, 40);
    assert_eq!(opaque[3], 255);
    assert!(
        (half[3] as i32 - 128).abs() <= 2,
        "[{label}] half alpha {half:?}"
    );
    for (name, got, want) in [
        ("opaque", opaque, expected_at(40, 40)),
        ("half", half, expected_at(200, 40)),
    ] {
        for channel in 0..4 {
            assert!(
                (got[channel] as i32 - want[channel] as i32).abs() <= 3,
                "[{label}] {name} quad {got:?} differs from the oracle {want:?}"
            );
        }
    }

    for _ in 0..6 {
        render(&mut backend, id, &plan);
        settle();
    }
    let outcome = window.backdrop_attribute();
    eprintln!("[{label}] DWM backdrop attribute {outcome:?}");

    match capture_client(window, extent) {
        None => eprintln!("[{label}] PrintWindow capture unavailable"),
        Some(capture) => {
            let solid = capture.pixel(40, 40);
            let blend = capture.pixel(200, 40);
            let open = capture.pixel(300, 200);
            let corner = capture.pixel(extent.width as usize - 2, extent.height as usize - 2);
            eprintln!(
                "[{label}] captured opaque {solid:?} half {blend:?} transparent {open:?} corner {corner:?}"
            );
            for channel in 0..3 {
                assert!(
                    (solid[channel] as i32 - SOLID[channel] as i32).abs() <= 3,
                    "[{label}] composed opaque quad {solid:?}"
                );
            }
            assert_ne!(
                blend[..3],
                solid[..3],
                "[{label}] a half-alpha quad must let the backdrop through"
            );
            assert!(
                open[..3].iter().any(|channel| *channel > 8),
                "[{label}] transparent swapchain pixels must show the backdrop material, got {open:?}"
            );
            assert_eq!(
                corner, open,
                "[{label}] the material must be uniform across the transparent area"
            );

            let blackout = opaque_black_scene(extent);
            for _ in 0..4 {
                render(&mut backend, id, &blackout);
                settle();
            }
            let covered = capture_client(window, extent)
                .expect("second capture")
                .pixel(300, 200);
            eprintln!("[{label}] opaque black clear covers the material: {covered:?}");
            assert!(
                covered[..3].iter().all(|channel| *channel <= 3),
                "[{label}] an opaque clear must hide the backdrop, got {covered:?}"
            );
        }
    }
    window.destroy();
}

#[test]
fn hardware_mica_composes_premultiplied_alpha() {
    composes(
        AdapterPreference::HighPerformance,
        WindowMaterial::Mica,
        "hardware-mica",
    );
}

#[test]
fn hardware_mica_alt_composes_premultiplied_alpha() {
    composes(
        AdapterPreference::HighPerformance,
        WindowMaterial::Tabbed,
        "hardware-mica-alt",
    );
}

#[test]
fn hardware_acrylic_composes_premultiplied_alpha() {
    composes(
        AdapterPreference::HighPerformance,
        WindowMaterial::Acrylic,
        "hardware-acrylic",
    );
}

#[test]
fn warp_mica_composes_premultiplied_alpha() {
    composes(AdapterPreference::Warp, WindowMaterial::Mica, "warp-mica");
}

#[test]
fn composition_surface_survives_resize_and_device_loss() {
    crate::support::hold_software();
    let mut backend = WindowBackend::new(options(AdapterPreference::HighPerformance))
        .unwrap_or_else(|error| panic!("no hardware adapter: {error}"));
    let (window, id, extent) = open_composed(&mut backend, WindowMaterial::Mica);
    let plan = translucent_scene(extent);
    for _ in 0..3 {
        render(&mut backend, id, &plan);
        settle();
    }
    window.set_logical_size(600.0, 380.0);
    settle();
    let (width, height) = window.inner_size();
    let resized = Extent { width, height };
    assert_ne!(resized, extent);
    backend.resize_surface(id, resized).unwrap();
    let plan = translucent_scene(resized);
    for _ in 0..3 {
        render(&mut backend, id, &plan);
        settle();
    }
    backend.inject_device_removal().unwrap();
    assert!(backend.execute(&plan, &FrameInputs::new(id)).is_err());
    backend.recover().unwrap();
    assert_eq!(backend.surface_kind(id), Some(SurfaceKind::Composition));
    let content = backend.composition_content(id).unwrap();
    window
        .attach_composition_surface(&content)
        .expect("re-attach after recovery");
    for _ in 0..3 {
        render(&mut backend, id, &plan);
        settle();
    }
    let capture = capture_client(window, resized);
    if let Some(capture) = capture {
        let solid = capture.pixel(40, 40);
        assert!((solid[2] as i32 - SOLID[2] as i32).abs() <= 3, "{solid:?}");
    }
    window.destroy();
}
