#[cfg(windows)]
mod native {
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;

    use aqua::config::AquaConfig;
    use aqua::render::pressure::watch_notification;
    use aqua::runtime::{AppCtx, Application, Aqua};
    use aqua_core::{DisplayItem, InputEvent, NodeId, Oklch, Rect};
    use aqua_protocol::geometry::{AquaWindowId, LogicalSize, PhysicalPosition};
    use aqua_protocol::window::{WindowOptions, WindowRequest};
    use aqua_protocol::{Instant, ObservedWindowState, WindowEvent};
    use aqua_scene::Scene;
    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };

    const WARM_FRAMES: u64 = 40;

    fn raise(id: AquaWindowId) {
        let _ = unsafe {
            SetWindowPos(
                HWND(id.raw() as *mut _),
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
    }

    fn block(x: f32, color: &str) -> DisplayItem {
        DisplayItem::Rect {
            bounds: Rect {
                x,
                y: 10.0,
                w: 24.0,
                h: 24.0,
            },
            radius: 4.0,
            fill: Oklch::from_hex(color).unwrap(),
            border_width: 0.0,
            border_color: Oklch::TRANSPARENT,
            shadow: None,
        }
    }

    #[derive(Debug)]
    enum Phase {
        Warm,
        Pressure { before: u64, since: Instant },
        Released { before: u64, since: Instant },
        Covering { before: u64 },
        Uncovering,
        Done,
    }

    struct Probe {
        scene: Scene,
        mover: NodeId,
        step: f32,
        main: Option<AquaWindowId>,
        cover: Option<AquaWindowId>,
        frames: u64,
        phase: Phase,
        notification: HANDLE,
        messages: aqua::mailbox::Mailbox<()>,
        progress: std::rc::Rc<std::cell::RefCell<String>>,
        verdict: std::rc::Rc<std::cell::RefCell<Option<Result<String, String>>>>,
    }

    impl Probe {
        fn memory_is_low(&self, low: bool) {
            let _ = match low {
                true => unsafe { SetEvent(self.notification) },
                false => unsafe { ResetEvent(self.notification) },
            };
        }

        fn trims(&self, ctx: &AppCtx) -> u64 {
            self.main
                .and_then(|id| ctx.render_stats(id))
                .map_or(0, |stats| stats.trims.load(Relaxed))
        }

        fn finish(&mut self, ctx: &mut AppCtx, verdict: Result<String, String>) {
            *self.verdict.borrow_mut() = Some(verdict);
            self.phase = Phase::Done;
            ctx.quit();
        }

        fn occluded(&self, ctx: &AppCtx) -> bool {
            self.main
                .and_then(|id| ctx.window_state(id))
                .is_some_and(|state| state.occluded)
        }
    }

    impl Application for Probe {
        fn on_start(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
            match self.main {
                None => {
                    self.main = Some(id);
                    ctx.bind_mailbox(id, &self.messages);
                    let sender = self.messages.sender();
                    std::thread::spawn(move || {
                        while !matches!(sender.send(()), Err(aqua::mailbox::SendError::Closed(())))
                        {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    });
                    raise(id);
                    ctx.request_window(WindowRequest::SetOuterPosition(
                        id,
                        PhysicalPosition { x: 150, y: 150 },
                    ))
                    .unwrap();
                }
                Some(_) => {
                    self.cover = Some(id);
                    raise(id);
                    ctx.request_window(WindowRequest::SetOuterPosition(
                        id,
                        PhysicalPosition { x: 100, y: 100 },
                    ))
                    .unwrap();
                }
            }
        }

        fn on_window_event(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &WindowEvent) {}

        fn on_input(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &InputEvent) {}

        fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
            if self.cover == Some(window.id) {
                return None;
            }
            self.step += 3.0;
            self.scene
                .update(self.mover, block(10.0 + self.step % 300.0, "#ffaa33"));
            Some(now + Duration::from_millis(8))
        }

        fn scene_mut(&mut self, id: AquaWindowId) -> Option<&mut Scene> {
            (self.cover != Some(id)).then_some(&mut self.scene)
        }

        fn on_frame_completed(&mut self, ctx: &mut AppCtx, id: AquaWindowId, _sequence: u64) {
            if self.main != Some(id) {
                return;
            }
            self.frames += 1;
            self.step(ctx);
        }

        fn on_wake(&mut self, ctx: &mut AppCtx, _id: AquaWindowId) {
            self.messages.drain();
            self.step(ctx);
        }
    }

    impl Probe {
        fn step(&mut self, ctx: &mut AppCtx) {
            let now = ctx.now();
            let trims = self.trims(ctx);
            *self.progress.borrow_mut() = format!(
                "{:?} trims={trims} occluded={} frames={}",
                self.phase,
                self.occluded(ctx),
                self.frames
            );
            let waited = |since: Instant| now.saturating_duration_since(since);
            match &self.phase {
                Phase::Warm if self.frames >= WARM_FRAMES => {
                    self.memory_is_low(true);
                    self.phase = Phase::Pressure {
                        before: trims,
                        since: now,
                    };
                }
                Phase::Pressure { before, since } => {
                    let (before, since) = (*before, *since);
                    if trims > before && waited(since) >= Duration::from_secs(4) {
                        match trims == before + 1 {
                            true => {
                                self.memory_is_low(false);
                                self.phase = Phase::Released {
                                    before: trims,
                                    since: now,
                                };
                            }
                            false => self.finish(
                                ctx,
                                Err(format!(
                                    "sustained pressure trimmed {} times",
                                    trims - before
                                )),
                            ),
                        }
                    } else if waited(since) > Duration::from_secs(12) {
                        self.finish(ctx, Err("low-memory pressure never reached a trim".into()));
                    }
                }
                Phase::Released { before, since } => {
                    let (before, since) = (*before, *since);
                    if waited(since) >= Duration::from_secs(3) {
                        match trims == before {
                            true => {
                                ctx.request_window(WindowRequest::Create(WindowOptions {
                                    title: "aqua trim cover".into(),
                                    inner_size: LogicalSize::new(1400.0, 1000.0),
                                    ..WindowOptions::default()
                                }))
                                .unwrap();
                                self.phase = Phase::Covering { before: trims };
                            }
                            false => {
                                self.finish(ctx, Err("trimming continued after release".into()))
                            }
                        }
                    }
                }
                Phase::Covering { before } => {
                    let before = *before;
                    if self.occluded(ctx) && trims > before {
                        if let Some(cover) = self.cover {
                            ctx.request_window(WindowRequest::Close(cover)).unwrap();
                        }
                        self.phase = Phase::Uncovering;
                    }
                }
                Phase::Uncovering if !self.occluded(ctx) => {
                    self.finish(
                    ctx,
                    Ok(format!(
                        "a signalled memory resource notification trimmed once; another window occluded and then exposed the main window; {trims} trims"
                    )),
                );
                }
                _ => {}
            }
        }
    }

    pub fn run() {
        unsafe { std::env::set_var("AQUA_SMOKE_MS", "60000") };
        let verdict = std::rc::Rc::new(std::cell::RefCell::new(None));
        let progress = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let mut config = AquaConfig::default();
        config.window.title = "aqua trim main".into();
        let notification =
            unsafe { CreateEventW(None, true, false, None) }.expect("memory notification event");
        watch_notification(Some(notification));
        let aqua = Aqua::new(config).expect("aqua");
        let mut scene = Scene::new();
        let mover = scene.insert(block(10.0, "#ffaa33"));
        let probe = Probe {
            scene,
            mover,
            step: 0.0,
            main: None,
            cover: None,
            frames: 0,
            phase: Phase::Warm,
            notification,
            messages: aqua::mailbox::Mailbox::new(std::num::NonZeroUsize::new(4).unwrap()),
            progress: progress.clone(),
            verdict: verdict.clone(),
        };
        if let Err(error) = aqua.run(probe) {
            eprintln!("host_trim: run failed: {error}");
            std::process::exit(1);
        }
        let outcome = verdict.borrow_mut().take();
        match outcome {
            Some(Ok(report)) => eprintln!("{report}\nhost_trim: ok"),
            Some(Err(message)) => {
                eprintln!("host_trim: FAILED: {message}");
                std::process::exit(1);
            }
            None => {
                eprintln!(
                    "host_trim: FAILED: no verdict before the host exited; last state {}",
                    progress.borrow()
                );
                std::process::exit(1);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    native::run();
    #[cfg(not(windows))]
    eprintln!("host_trim: skipped; requires Windows");
}
