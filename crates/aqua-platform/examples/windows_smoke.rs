//! Windows smoke helper for `aqua-platform` (no facade overlap).
//!
//! Launch later under PowerShell on a real Windows 11 host:
//! `cargo run -p aqua-platform --example windows_smoke`.
//! It opens one window through the PUBLIC platform API only
//! (`host::window_attributes`, `Scheduler`, materials, prefs, dialogs
//! capability, a11y attach), prints capability lines, presents a frame
//! budget, then exits. Never run headless in CI — it opens a real window.
//!
//! Env: `AQUA_SMOKE_MATERIAL=mica|mica-alt|acrylic|none` (default mica),
//! `AQUA_SMOKE_FRAMES=N` (default 120).

use std::num::NonZeroU32;
use std::time::{Duration, Instant as WallInstant};

#[cfg(windows)]
use aqua_platform::material;
#[cfg(not(windows))]
use aqua_platform::{a11y, dialogs};
use aqua_platform::{host, prefs, window, AquaWindowId, FrameReason};
use aqua_protocol::Instant;
use aqua_runtime::Scheduler;

#[cfg(windows)]
fn parse_material() -> material::WindowMaterial {
    match std::env::var("AQUA_SMOKE_MATERIAL")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "mica-alt" | "micaalt" => material::WindowMaterial::MicaAlt,
        "acrylic" => material::WindowMaterial::Acrylic,
        "none" => material::WindowMaterial::None,
        _ => material::WindowMaterial::Mica,
    }
}

struct Smoke {
    id: AquaWindowId,
    window: Option<std::sync::Arc<winit::window::Window>>,
    #[cfg(windows)]
    proxy: winit::event_loop::EventLoopProxy<()>,
    scheduler: Scheduler,
    epoch: WallInstant,
    frames: u32,
    budget: u32,
    #[cfg(windows)]
    a11y: Option<aqua_platform::a11y_winit::A11yAdapter>,
}

impl Smoke {
    fn now(&self) -> Instant {
        Instant::from_nanos(u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }

    fn wall(&self, at: Instant) -> WallInstant {
        self.epoch + at.since_origin()
    }

    fn draw(&mut self) {
        let now = self.now();
        let ready = self.scheduler.ready_windows(now);
        if ready.is_empty() {
            return;
        }
        let ids: Vec<AquaWindowId> = ready.into_iter().map(|(id, _)| id).collect();
        self.scheduler.mark_produced(&ids, now);
        for id in &ids {
            self.scheduler.mark_presented(*id);
        }
        self.frames += ids.len() as u32;
        self.scheduler
            .request_animation_deadline(self.id, now + Duration::from_millis(16));
    }
}

impl winit::application::ApplicationHandler for Smoke {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let opts = window::WindowOptions {
            title: "aqua platform smoke".to_string(),
            transparent: true,
            ..Default::default()
        };
        let attrs = host::window_attributes(&opts).with_visible(false);
        let window = std::sync::Arc::new(
            event_loop
                .create_window(attrs)
                .expect("smoke: create_window failed"),
        );

        #[cfg(windows)]
        {
            let waker = {
                let proxy = self.proxy.clone();
                std::sync::Arc::new(move || {
                    let _ = proxy.send_event(());
                })
            };
            match aqua_platform::a11y_winit::A11yAdapter::attach_with_waker(
                event_loop, &window, waker,
            ) {
                Ok(a) => {
                    println!("smoke: a11y attached");
                    self.a11y = Some(a);
                }
                Err(e) => println!("smoke: a11y attach failed: {e:?}"),
            }
            let requested = parse_material();
            let prefs = prefs::query_preferences();
            let selected = material::select_material(requested, &prefs);
            println!("smoke: prefs={prefs:?} material={selected:?}");
            match aqua_platform::adapters::apply_material(&window, selected) {
                Ok(cap) => println!("smoke: material capability={cap:?}"),
                Err(e) => println!("smoke: material failed, opaque fallback: {e:?}"),
            }
            match aqua_platform::adapters::windows::apply_titlebar_theme_to_window(&window, true) {
                Ok(()) => println!("smoke: dark titlebar applied"),
                Err(e) => println!("smoke: titlebar theme failed: {e:?}"),
            }
        }
        #[cfg(not(windows))]
        {
            println!("smoke: prefs={:?}", prefs::query_preferences());
            println!("smoke: dialogs={:?}", dialogs::dialogs_capability());
            println!("smoke: a11y={:?}", a11y::a11y_capability());
        }

        window.set_visible(true);
        window.request_redraw();
        let now = self.now();
        self.scheduler.request_animation_deadline(self.id, now);
        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let _ = window_id;
        #[cfg(windows)]
        if let Some(w) = &self.window {
            if let Some(a) = self.a11y.as_mut() {
                a.process_window_event(w, &event);
            }
        }
        let scale = self
            .window
            .as_ref()
            .map(|w| w.scale_factor())
            .unwrap_or(1.0);
        if let Some(fw) = host::translate_window_event(&event, self.id, scale) {
            if let Some(reason) = aqua_platform::events::window_event_frame_reason(&fw) {
                self.scheduler.request_frame(self.id, reason);
            }
            let sig = host::surface_signal(&fw);
            if sig.skip_production {
                return;
            }
            if matches!(fw, window::WindowEvent::CloseRequested(_)) {
                event_loop.exit();
                return;
            }
        }
        if matches!(event, winit::event::WindowEvent::RedrawRequested) {
            self.draw();
            if self.frames >= self.budget {
                println!("smoke: budget reached ({} frames)", self.frames);
                event_loop.exit();
                return;
            }
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
        let next = self
            .scheduler
            .next_wakeup(self.now())
            .map(|at| self.wall(at));
        event_loop.set_control_flow(host::control_flow_for_next_wakeup(next));
    }

    fn user_event(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop, _event: ()) {
        // Event-loop-proxy wake (a11y bridge): drain AT actions (none wired
        // in smoke) and ensure a dispatch happens while idle.
        #[cfg(windows)]
        if let Some(a) = self.a11y.as_mut() {
            let _ = a.drain_actions();
        }
        self.scheduler.request_frame(self.id, FrameReason::EXPLICIT);
    }
}

fn main() {
    let budget: u32 = std::env::var("AQUA_SMOKE_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    let event_loop = winit::event_loop::EventLoop::new().expect("smoke: event loop");
    #[cfg(windows)]
    let proxy = event_loop.create_proxy();
    let mut scheduler = Scheduler::new();
    scheduler.set_fps_target(NonZeroU32::new(60));
    let mut app = Smoke {
        id: AquaWindowId::new(1).unwrap(),
        window: None,
        #[cfg(windows)]
        proxy,
        scheduler,
        epoch: WallInstant::now(),
        frames: 0,
        budget,
        #[cfg(windows)]
        a11y: None,
    };
    event_loop.run_app(&mut app).expect("smoke: run_app failed");
}
