#[cfg(windows)]
mod native {

    use aqua::mailbox::Mailbox;
    use aqua::{AppCtx, Application, Aqua, AquaConfig, Instant};
    use aqua_core::scene::{SnapshotTransition, TextRasterMode};
    use aqua_core::{DisplayItem, InputEvent, Oklch, Rect};
    use aqua_protocol::geometry::LogicalSize;
    use aqua_protocol::window::{WindowError, WindowOptions};
    use aqua_protocol::TaskbarBadge;
    use aqua_protocol::{AquaWindowId, ObservedWindowState, WindowEvent, WindowRequest};
    use aqua_scene::Scene;
    use std::cell::RefCell;
    use std::num::NonZeroUsize;
    use std::rc::Rc;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;

    #[derive(Default)]
    struct Report {
        started: Vec<AquaWindowId>,
        resized: bool,
        child_closed: bool,
        vetoed: bool,
        delivered: bool,
        display: bool,
        title_observed: bool,
        passed: bool,
        initial_scene_built: bool,
        resized_at: Option<Instant>,
        errors: Vec<WindowError>,
    }

    enum Phase {
        Setup,
        Transitioning { started: Instant, builds: u32 },
        Finished,
    }

    struct Desktop {
        report: Rc<RefCell<Report>>,
        messages: Mailbox<()>,
        main: Option<AquaWindowId>,
        scene: Scene,
        phase: Phase,
        stop_worker: Arc<AtomicBool>,
    }

    impl Application for Desktop {
        fn on_start(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
            self.report.borrow_mut().started.push(id);
            if self.main.is_some() {
                ctx.request_window(WindowRequest::Close(id)).unwrap();
                return;
            }
            self.main = Some(id);
            self.report.borrow_mut().display = ctx.display(id).is_some_and(|display| {
                display.physical_work_area.w > 0.0 && display.scale_factor > 0.0
            });
            ctx.bind_mailbox(id, &self.messages);
            let watchdog = self.messages.sender();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(12));
                let _ = watchdog.send(());
            });
            let sender = self.messages.sender();
            let stop = self.stop_worker.clone();
            std::thread::spawn(move || {
                for _ in 0..500 {
                    std::thread::sleep(Duration::from_millis(20));
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    match sender.send(()) {
                        Ok(()) | Err(aqua::mailbox::SendError::Full(())) => {}
                        Err(aqua::mailbox::SendError::Closed(())) => break,
                    }
                }
            });
            for request in [
                WindowRequest::Close(id),
                WindowRequest::SetMaxFrameRate(id, std::num::NonZeroU32::new(60)),
                WindowRequest::SetVsync(id, false),
                WindowRequest::SetVsync(id, true),
                WindowRequest::SetTitle(id, "Aqua desktop contract changed".into()),
                WindowRequest::SetInnerSize(id, LogicalSize::new(640.0, 480.0)),
                WindowRequest::SetBadge(
                    id,
                    Some(TaskbarBadge {
                        count: 3,
                        color: Oklch::from_hex("#286cff").unwrap(),
                    }),
                ),
                WindowRequest::SetBadge(id, None),
                WindowRequest::Create(WindowOptions {
                    title: "Aqua desktop child".into(),
                    ..Default::default()
                }),
            ] {
                ctx.request_window(request).unwrap();
            }
        }

        fn can_close(&mut self, _: &mut AppCtx, id: AquaWindowId) -> bool {
            if self.main == Some(id) {
                self.report.borrow_mut().vetoed = true;
                false
            } else {
                true
            }
        }

        fn on_window_event(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
            match event {
                WindowEvent::Resized(_, size)
                    if self.main == Some(id)
                        && (size.width - 640.0).abs() < 2.0
                        && (size.height - 480.0).abs() < 2.0 =>
                {
                    self.report.borrow_mut().resized = true;
                    self.report.borrow_mut().resized_at = Some(ctx.now());
                }
                WindowEvent::Destroyed(_) if self.main != Some(id) => {
                    self.report.borrow_mut().child_closed = true
                }
                _ => {}
            }
        }

        fn on_input(&mut self, _: &mut AppCtx, _: AquaWindowId, _: &InputEvent) {}

        fn on_wake(&mut self, ctx: &mut AppCtx, _: AquaWindowId) {
            let delivered = !self.messages.drain().is_empty();
            let mut report = self.report.borrow_mut();
            report.delivered |= delivered;
            report.title_observed |= self
                .main
                .and_then(|id| ctx.window_state(id))
                .is_some_and(|state| state.title == "Aqua desktop contract changed");
            if !report.errors.is_empty() || ctx.now().since_origin() > Duration::from_secs(10) {
                ctx.quit();
                return;
            }
            if report.resized_at.is_some_and(|time| {
                ctx.now().saturating_duration_since(time) >= Duration::from_millis(150)
            }) && report.initial_scene_built
                && report.resized
                && report.child_closed
                && report.vetoed
                && report.delivered
            {
                match self.phase {
                    Phase::Setup => {
                        ctx.request_window(WindowRequest::BeginSnapshotTransition(
                            self.main.unwrap(),
                            SnapshotTransition {
                                duration: Duration::from_millis(80),
                                band: 0.18,
                                shimmer: Oklch::from_hex("#ffffff").unwrap(),
                            },
                        ))
                        .unwrap();
                        ctx.request_window(WindowRequest::SetTextRasterMode(
                            self.main.unwrap(),
                            TextRasterMode::Outline,
                        ))
                        .unwrap();
                        ctx.request_window(WindowRequest::SetTextRasterMode(
                            self.main.unwrap(),
                            TextRasterMode::PlatformDefault,
                        ))
                        .unwrap();
                        self.phase = Phase::Transitioning {
                            started: ctx.now(),
                            builds: 0,
                        };
                        self.stop_worker.store(true, Ordering::SeqCst);
                        let sender = self.messages.sender();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(120));
                            let _ = sender.send(());
                        });
                    }
                    Phase::Finished => {
                        report.passed = true;
                        ctx.quit();
                    }
                    Phase::Transitioning { started, .. } => {
                        if ctx.now().saturating_duration_since(started) >= Duration::from_millis(80)
                        {
                            report.passed = true;
                            ctx.quit();
                        }
                    }
                }
            } else if ctx.now().since_origin() > Duration::from_secs(10) {
                ctx.quit();
            }
        }

        fn on_window_request_error(&mut self, _request: &WindowRequest, error: &WindowError) {
            eprintln!("WINDOW_REQUEST_ERROR: {error:?}");
            self.report.borrow_mut().errors.push(error.clone());
            let _ = self.messages.sender().send(());
        }

        fn on_render_error(&mut self, ctx: &mut AppCtx, _: AquaWindowId, message: &str) {
            self.report
                .borrow_mut()
                .errors
                .push(WindowError::Native(message.into()));
            ctx.quit();
        }

        fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
            if self.main != Some(window.id) {
                return None;
            }
            if matches!(self.phase, Phase::Setup)
                && (window.inner_size.width - 640.0).abs() < 2.0
                && (window.inner_size.height - 480.0).abs() < 2.0
            {
                self.report.borrow_mut().initial_scene_built = true;
            }
            let transitioned = !matches!(self.phase, Phase::Setup);
            let fill = Oklch::from_hex(if transitioned { "#286cff" } else { "#dc3838" }).unwrap();
            let item = DisplayItem::Rect {
                bounds: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 640.0,
                    h: 480.0,
                },
                radius: 0.0,
                fill,
                border_width: 0.0,
                border_color: fill,
                shadow: None,
            };
            let first = self.scene.ordered().next().map(|(id, _)| id);
            match first {
                Some(id) => {
                    self.scene.update(id, item);
                }
                None => {
                    self.scene.insert(item);
                }
            }
            if let Phase::Transitioning { started, builds } = &mut self.phase {
                *builds += 1;
                if now.saturating_duration_since(*started) >= Duration::from_millis(80) {
                    self.phase = Phase::Finished;
                    self.messages.sender().send(()).unwrap();
                }
            }
            None
        }

        fn scene_mut(&mut self, id: AquaWindowId) -> Option<&mut Scene> {
            (self.main == Some(id)).then_some(&mut self.scene)
        }
    }

    pub fn run() {
        std::env::set_var("AQUA_RENDER_WHEN_COVERED", "1");
        let report = Rc::new(RefCell::new(Report::default()));
        let mut config = AquaConfig::default();
        config.window.title = "Aqua desktop contract".into();
        Aqua::new(config)
            .unwrap()
            .run(Desktop {
                report: report.clone(),
                messages: Mailbox::new(NonZeroUsize::new(32).unwrap()),
                main: None,
                scene: Scene::new(),
                phase: Phase::Setup,
                stop_worker: Arc::new(AtomicBool::new(false)),
            })
            .unwrap();
        let report = report.borrow();
        assert_eq!(report.started.len(), 2);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(
            report.passed && report.display && report.title_observed,
            "resized={} child_closed={} vetoed={} delivered={} initial_scene_built={} display={} title_observed={}",
            report.resized,
            report.child_closed,
            report.vetoed,
            report.delivered,
            report.initial_scene_built,
            report.display,
            report.title_observed
        );
        println!(
            "WINDOWS_DESKTOP_OK: worker wake, native resize, taskbar badges, two-window lifecycle, close veto, snapshot transition and text raster requests accepted"
        );
    }
}

fn main() {
    #[cfg(windows)]
    native::run();
}
