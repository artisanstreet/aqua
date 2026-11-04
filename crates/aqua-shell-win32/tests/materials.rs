#![cfg(windows)]

use aqua_protocol::WindowMaterial;
use aqua_shell_win32::{Window, WindowOptions};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISwapChain1, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

static DESKTOP: Mutex<()> = Mutex::new(());

fn pump(window: Option<Window>, behind: Option<(Window, u32, u32, &[u8])>, duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if let Some((window, width, height, pixels)) = &behind {
            window.present_bgra(*width, *height, pixels);
        }
        if let Some(window) = window {
            window.request_redraw();
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

fn panel(position: (i32, i32), size: (f64, f64), material: WindowMaterial) -> Window {
    Window::create(&WindowOptions {
        title: "aqua-material-test".into(),
        logical_size: size,
        position: Some(position),
        visible: false,
        alpha_surface: material != WindowMaterial::Solid,
        material,
        dark: true,
        ..WindowOptions::panel(size)
    })
    .expect("panel")
}

struct Chain {
    _device: ID3D11Device,
    chain: IDXGISwapChain1,
}

impl Chain {
    fn clear_and_present(&self) {
        use windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView;
        let buffer: ID3D11Texture2D = unsafe { self.chain.GetBuffer(0) }.expect("buffer");
        let mut view: Option<ID3D11RenderTargetView> = None;
        unsafe { self._device.CreateRenderTargetView(&buffer, None, Some(&mut view)) }
            .expect("rtv");
        let context = unsafe { self._device.GetImmediateContext() }.expect("context");
        unsafe { context.ClearRenderTargetView(view.as_ref().expect("view"), &[0.0; 4]) };
        unsafe { self.chain.Present(0, Default::default()) }
            .ok()
            .expect("present");
    }
}

fn attach_transparent(window: Window) -> Chain {
    let mut device: Option<ID3D11Device> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    }
    .expect("d3d11");
    let device = device.expect("device");
    let dxgi: IDXGIDevice = device.cast().expect("dxgi");
    let factory: IDXGIFactory2 = unsafe {
        dxgi.GetAdapter()
            .expect("adapter")
            .GetParent()
            .expect("factory")
    };
    let (width, height) = window.inner_size();
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: width,
        Height: height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        ..Default::default()
    };
    let chain = unsafe { factory.CreateSwapChainForComposition(&device, &desc, None) }
        .expect("swapchain");
    let chain = Chain {
        _device: device,
        chain,
    };
    chain.clear_and_present();
    let surface: windows::core::IUnknown = chain.chain.cast().expect("unknown");
    window.attach_composition_surface(&surface).expect("attach");
    chain
}

struct Shot {
    width: i32,
    height: i32,
    bgra: Vec<u8>,
}

fn capture(x: i32, y: i32, width: i32, height: i32) -> Shot {
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap.into());
        BitBlt(
            memory,
            0,
            0,
            width,
            height,
            Some(screen),
            x,
            y,
            SRCCOPY | CAPTUREBLT,
        )
        .expect("blt");
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        Shot {
            width,
            height,
            bgra,
        }
    }
}

impl Shot {
    fn mean(&self, x: i32, y: i32, width: i32, height: i32) -> [f64; 3] {
        let mut sum = [0.0f64; 3];
        for row in y..y + height {
            for column in x..x + width {
                let at = ((row * self.width + column) * 4) as usize;
                sum[0] += f64::from(self.bgra[at + 2]);
                sum[1] += f64::from(self.bgra[at + 1]);
                sum[2] += f64::from(self.bgra[at]);
            }
        }
        let count = f64::from(width * height);
        sum.map(|channel| channel / count)
    }

    fn save_bmp(&self, name: &str) {
        let Ok(directory) = std::env::var("AQUA_SHOT_DIR") else {
            return;
        };
        let mut file = Vec::with_capacity(54 + self.bgra.len());
        let size = 54 + self.bgra.len() as u32;
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&size.to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend_from_slice(&40u32.to_le_bytes());
        file.extend_from_slice(&self.width.to_le_bytes());
        file.extend_from_slice(&(-self.height).to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&[0; 24]);
        file.extend_from_slice(&self.bgra);
        let _ = std::fs::write(format!("{directory}/{name}.bmp"), file);
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(a, b)| (a - b).abs()).sum::<f64>() / 3.0
}

struct Backdrop {
    window: Window,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn split_backdrop(origin: (i32, i32)) -> Backdrop {
    let window = panel(origin, (640.0, 420.0), WindowMaterial::Solid);
    let (width, height) = window.inner_size();
    let pixels: Vec<u8> = (0..height)
        .flat_map(|_| {
            (0..width).flat_map(move |column| match column < width / 2 {
                true => [255, 255, 255, 255],
                false => [0, 0, 0, 255],
            })
        })
        .collect();
    window.show();
    Backdrop {
        window,
        width,
        height,
        pixels,
    }
}

struct Measurement {
    left: [f64; 3],
    right: [f64; 3],
}

fn measure(material: WindowMaterial, name: &str) -> Measurement {
    let scout = panel((0, 0), (10.0, 10.0), WindowMaterial::Solid);
    let monitor = scout.monitor().expect("monitor");
    scout.destroy();
    let (left, top, _, _) = monitor.work_area;
    let origin = (left + 160, top + 160);
    let backdrop = split_backdrop(origin);
    let (bw, bh) = (backdrop.width as i32, backdrop.height as i32);
    let probe = panel((0, 0), (320.0, 200.0), WindowMaterial::Solid);
    let (pw, ph) = probe.inner_size();
    probe.destroy();
    let x = origin.0 + (bw - pw as i32) / 2;
    let y = origin.1 + (bh - ph as i32) / 2;
    let window = panel((x, y), (320.0, 200.0), material);
    let chain = attach_transparent(window);
    window.show();
    let behind = || Some((backdrop.window, backdrop.width, backdrop.height, &backdrop.pixels[..]));
    pump(Some(window), behind(), Duration::from_millis(400));
    chain.clear_and_present();
    pump(Some(window), behind(), Duration::from_millis(1200));
    let (pw, ph) = window.inner_size();
    let outcome = window.set_material(material);
    assert_eq!(outcome.active, material, "{outcome:?}");
    assert!(window.is_visible());
    let sample = (pw as i32) / 5;
    let shot = capture(x, y, pw as i32, ph as i32);
    shot.save_bmp(name);
    capture(origin.0, origin.1, bw, bh).save_bmp(&format!("{name}-context"));
    let band = ph as i32 / 3;
    let measurement = Measurement {
        left: shot.mean(sample / 2, band, sample, band),
        right: shot.mean(pw as i32 - sample - sample / 2, band, sample, band),
    };
    println!(
        "{name}: left {:?} right {:?} backdrop attribute {:?}",
        measurement.left,
        measurement.right,
        window.backdrop_attribute()
    );
    window.destroy();
    backdrop.window.destroy();
    measurement
}

#[test]
fn mica_mica_alt_and_acrylic_pixels_follow_the_system_backdrop_contract() {
    let _desktop = DESKTOP.lock().unwrap_or_else(|p| p.into_inner());
    let mica = measure(WindowMaterial::Mica, "mica");
    let alt = measure(WindowMaterial::Tabbed, "mica-alt");
    let acrylic = measure(WindowMaterial::Acrylic, "acrylic");

    let spread = |m: &Measurement| distance(m.left, m.right);
    [("mica", &mica), ("mica alt", &alt), ("acrylic", &acrylic)]
        .iter()
        .for_each(|(name, m)| {
            assert!(
                spread(m) < 8.0,
                "{name} is flat while inactive: {:?} vs {:?}",
                m.left,
                m.right
            );
            assert!(
                distance(m.left, [0.0; 3]) > 3.0,
                "{name} must paint a backdrop, got {:?}",
                m.left
            );
        });
    assert!(
        distance(mica.left, acrylic.left) > 20.0,
        "acrylic and mica are distinct materials: {:?} vs {:?}",
        acrylic.left,
        mica.left
    );
    assert!(distance(mica.left, alt.left) < 8.0);
}

fn occlusion_loop(window: Window, expect_covered: bool, within: Duration) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        pump(None, None, Duration::from_millis(20));
        if window.is_covered() == expect_covered {
            return true;
        }
    }
    false
}

#[test]
fn coverage_by_other_windows_is_detected_and_reverts_when_they_move_away() {
    let _desktop = DESKTOP.lock().unwrap_or_else(|p| p.into_inner());
    let scout = panel((0, 0), (10.0, 10.0), WindowMaterial::Solid);
    let (left, top, _, _) = scout.monitor().expect("monitor").work_area;
    scout.destroy();
    let victim = panel((left + 200, top + 200), (300.0, 200.0), WindowMaterial::Solid);
    victim.show();
    pump(None, None, Duration::from_millis(300));
    assert!(!victim.is_covered());
    let blocker = panel((left + 100, top + 100), (900.0, 700.0), WindowMaterial::Solid);
    blocker.show();
    assert!(occlusion_loop(victim, true, Duration::from_secs(2)));
    blocker.set_position(left + 5000, top);
    pump(None, None, Duration::from_millis(100));
    assert!(occlusion_loop(victim, false, Duration::from_secs(2)));
    let half = panel((left + 200, top + 100), (300.0, 200.0), WindowMaterial::Solid);
    half.show();
    pump(None, None, Duration::from_millis(300));
    assert!(!victim.is_covered(), "a partial overlap leaves pixels visible");
    half.destroy();
    blocker.destroy();
    victim.destroy();
}
