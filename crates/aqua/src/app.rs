//! Root application builder: the canonical Aqua entry point.
//!
//! ```rust,no_run
//! # use aqua::{App, Config, Window, Theme, Material, px};
//! # fn todo_app(cx: &aqua::Context) -> impl aqua::IntoView {
//! #     aqua::ui::Text::new("tasks").build()
//! # }
//! # fn main() -> Result<(), aqua::Error> {
//! App::new()
//!     .config(
//!         Config::new().window(
//!             Window::new()
//!                 .title("Aqua — Tasks")
//!                 .size(px(760.0), px(560.0))
//!                 .min_size(px(480.0), px(360.0))
//!                 .theme(Theme::Dark)
//!                 .material(Material::Glass),
//!         ),
//!     )
//!     .run(todo_app)?;
//! #     Ok(())
//! # }
//! ```
//!
//! `App` is pure until [`App::run`]: attaching a [`Config`] stores it, and
//! all OS/GPU work happens inside `run`. The authoring builders (`Config`,
//! `Window`, `Theme`, `Material`) and the resolve seam (`ResolvedConfig`,
//! `ResolvedMaterial`, `ConfigError`) live in [`crate::config`] and are
//! consumed here through `Config::resolve(&OsPreferences) ->
//! Result<ResolvedConfig, ConfigError>` with `ResolvedConfig { runtime:
//! AquaConfig, material: ResolvedMaterial }`.

use aqua_platform::{
    prefs::{query_preferences, OsPreferences},
    AquaWindowId, ObservedWindowState,
};
use aqua_reactive::{DepTuple, Scope};
use aqua_render::Scene;
use aqua_ui::element::Element;

use crate::config::{Config, ResolvedConfig, ResolvedMaterial};
use crate::error::Error;
use crate::runtime::Aqua;
use crate::ui_app::{UiApp, UiAppError};

// ---------------------------------------------------------------------------
// Setup context + view vocabulary
// ---------------------------------------------------------------------------

/// Setup context handed to the [`App::run`] factory.
///
/// Gives the factory the root reactive [`Scope`] for signal setup, the
/// startup [`ResolvedMaterial`] for backdrop-aware styling, and the
/// [`Context::view`] constructor for explicit-dependency views.
pub struct Context<'a> {
    scope: &'a Scope,
    material: &'a ResolvedMaterial,
}

impl<'a> Context<'a> {
    /// Root reactive scope: create app signals here and pass them into
    /// [`Context::view`] as explicit dependencies.
    #[must_use]
    pub fn scope(&self) -> &Scope {
        self.scope
    }

    /// Startup material resolution for UI fallback styling
    /// (e.g. `if cx.material().is_glass() { ... }`): startup
    /// capability/policy snapshot, NOT proof the compositor applied
    /// anything (the window does not exist yet at setup time).
    #[must_use]
    pub fn material(&self) -> &ResolvedMaterial {
        self.material
    }

    /// Explicit-dependency view: `render` rebuilds the whole element
    /// whenever `deps` change (keyed reconciliation preserves nodes);
    /// targeted prop updates keep flowing through bindings. No hidden
    /// subscriptions — the dependency tuple is the whole subscription set.
    pub fn view<D, F>(&self, deps: D, render: F) -> ReactiveView<D, F>
    where
        D: DepTuple,
        F: FnMut(D::Values) -> Element + 'static,
    {
        ReactiveView { deps, render }
    }
}

/// Explicit-dependency view built by [`Context::view`]: the declared
/// dependency tuple plus the structural render closure.
pub struct ReactiveView<D, F> {
    deps: D,
    render: F,
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for aqua_ui::element::Element {}
    impl<
            D: aqua_reactive::DepTuple,
            F: FnMut(D::Values) -> aqua_ui::element::Element + 'static,
        > Sealed for super::ReactiveView<D, F>
    {
    }
}

/// What an [`App::run`] setup factory may return, implemented for exactly:
/// - [`Element`] (static content), and
/// - [`ReactiveView<D, F>`](ReactiveView) (explicit dependencies).
///
/// Sealed: no other implementations exist. Mounting shares the existing
/// [`UiApp::mount`] / [`UiApp::mount_reactive`] seams, scopes, and disposal
/// — no hidden dependencies, no alternate reactive graph.
pub trait IntoView: sealed::Sealed {
    /// Mount into a fresh host. The host must be unmounted.
    fn mount_into(self, ui: &mut UiApp) -> Result<(), UiAppError>;
}

impl IntoView for Element {
    fn mount_into(self, ui: &mut UiApp) -> Result<(), UiAppError> {
        ui.mount(|_| self)
    }
}

impl<D, F> IntoView for ReactiveView<D, F>
where
    D: DepTuple,
    F: FnMut(D::Values) -> Element + 'static,
{
    fn mount_into(self, ui: &mut UiApp) -> Result<(), UiAppError> {
        ui.mount_reactive(self.deps, self.render)
    }
}

// ---------------------------------------------------------------------------
// Root builder
// ---------------------------------------------------------------------------

/// Canonical application builder. Pure until [`App::run`].
#[derive(Debug, Clone, Default)]
pub struct App {
    config: Option<Config>,
}

impl App {
    /// Empty builder (default config unless [`App::config`] overrides).
    #[must_use]
    pub fn new() -> Self {
        Self { config: None }
    }

    /// Attach the authoring config. Pure: stored, not resolved.
    #[must_use]
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Resolve, mount, and run: exactly one OS-preferences snapshot feeds
    /// both [`Config::resolve`] and runtime construction
    /// (`Aqua::new_with_preferences`, crate-private), so startup never
    /// snapshots the OS twice. Legacy [`Aqua::new`] keeps querying itself
    /// for direct low-level use.
    ///
    /// Pipeline (shared with the headless tests through [`App::prepare`]):
    /// resolve → construct → reduced-motion policy + shared fonts/runs →
    /// setup factory → mount → GUI loop, or — when the resolved config is
    /// headless — the canonical headless one-frame lifecycle (mount →
    /// flush/layout/scene → dispose) instead of the loop. Invalid config
    /// never executes the factory. Any setup panic becomes [`Error::Init`]
    /// after disposing the host; any mount failure becomes [`Error::Mount`]
    /// after disposing the host.
    pub fn run<V>(self, setup: impl FnOnce(&Context) -> V) -> Result<(), Error>
    where
        V: IntoView,
    {
        let prefs = query_preferences();
        Self::prepare(self.config.unwrap_or_default(), &prefs, setup)?.run()
    }

    /// The exact preparation [`App::run`] executes: resolve → construct →
    /// motion policy → setup → mount. Internal tests drive this same
    /// function headless — there is no parallel fake pipeline.
    pub(crate) fn prepare<V>(
        config: Config,
        prefs: &OsPreferences,
        setup: impl FnOnce(&Context) -> V,
    ) -> Result<Prepared, Error>
    where
        V: IntoView,
    {
        let resolved = config.resolve(prefs).map_err(Error::Config)?;
        let aqua = Aqua::new_with_preferences(resolved.runtime.clone(), prefs.clone())
            .map_err(Error::Init)?;
        let mut ui = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
        ui.apply_startup_motion_policy(
            prefs.reduce_motion.is_on() || resolved.runtime.reduced_motion,
        );
        let view = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let ctx = Context {
                scope: ui.scope(),
                material: &resolved.material,
            };
            setup(&ctx)
        })) {
            Ok(view) => view,
            Err(payload) => {
                ui.dispose();
                return Err(Error::Init(panic_message(&payload)));
            }
        };
        if let Err(e) = view.mount_into(&mut ui) {
            ui.dispose();
            return Err(Error::Mount(e));
        }
        Ok(Prepared { aqua, ui, resolved })
    }
}

/// Resolved + mounted application, ready to enter the GUI loop or the
/// headless one-frame lifecycle. Built only by [`App::prepare`].
pub(crate) struct Prepared {
    aqua: Aqua,
    ui: UiApp,
    resolved: ResolvedConfig,
}

impl Prepared {
    fn run(self) -> Result<(), Error> {
        let Prepared {
            aqua,
            mut ui,
            resolved,
        } = self;
        if resolved.runtime.headless {
            return run_headless_one_frame(&mut ui, &resolved);
        }
        // On loop failure the running host drops with the loop, and
        // `UiApp::drop` funnels into `dispose` (scopes + views + runs).
        aqua.run(ui).map_err(Error::Runtime)?;
        Ok(())
    }
}

/// Canonical headless one-frame lifecycle: the mounted UI builds exactly one
/// frame (flush → layout → scene emission) against the configured window
/// size at scale 1.0, then the host is disposed.
///
/// This is a deterministic smoke snapshot, NOT a full long-running headless
/// app: no event loop runs, no worker-thread wake pump, no animation
/// deadlines advance. It exists so `.run(...)` is black-box testable with a
/// headless [`Config`] (unlike legacy [`Aqua::run`](Aqua::run), which
/// rejects headless configs because there is no event loop to enter).
fn run_headless_one_frame(ui: &mut UiApp, resolved: &ResolvedConfig) -> Result<(), Error> {
    let observed = ObservedWindowState::new(
        AquaWindowId::new(1).expect("headless window id"),
        resolved.runtime.window.title.clone(),
        resolved.runtime.window.inner_size,
    );
    let mut scene = Scene::new();
    let result = ui
        .build_scene_into(&mut scene, &observed)
        .map(|_| ())
        .map_err(UiAppError::Build)
        .map_err(Error::Mount);
    ui.dispose();
    result
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "setup factory panicked".to_string()
    }
}

// ---------------------------------------------------------------------------
// Internal tests (same prepare pipeline as run, headless)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use aqua_platform::prefs::PrefState;
    use aqua_platform::WindowMaterial;
    use aqua_reactive::{ReadSignal, WriteSignal};
    use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
    use aqua_ui::style::{pct, px, Style};
    use aqua_ui::tree::NodeKind;

    use crate::config::{Material, Theme, Window};

    fn test_window() -> Window {
        Window::new()
            .title("app-test")
            .size(px(760.0), px(560.0))
            .min_size(px(480.0), px(360.0))
            .theme(Theme::Dark)
            .material(Material::Glass)
    }

    fn hello_box(label: &str) -> Element {
        UiBox::new()
            .style(Style::new().width(px(200.0)).height(px(100.0)))
            .child(
                UiText::builder()
                    .arg(label)
                    .style(Style::new().width(px(120.0)).height(px(40.0)))
                    .build(),
            )
            .build()
    }

    fn texts(ui: &UiApp) -> Vec<String> {
        ui.tree()
            .traverse()
            .into_iter()
            .filter_map(|n| match ui.tree().get(n).map(|r| &r.kind) {
                Some(NodeKind::Text(t)) if !t.is_editable() && !t.text().is_empty() => {
                    Some(t.text().to_string())
                }
                _ => None,
            })
            .collect()
    }

    fn observed_for(resolved: &ResolvedConfig) -> ObservedWindowState {
        ObservedWindowState::new(
            AquaWindowId::new(1).expect("test window id"),
            resolved.runtime.window.title.clone(),
            resolved.runtime.window.inner_size,
        )
    }

    #[test]
    fn builders_are_pure_until_run() {
        // Storing invalid values must not panic; nothing runs without run().
        let _app = App::new().config(
            Config::new()
                .fps(0)
                .window(Window::new().size(px(0.0), px(-3.0))),
        );
    }

    #[test]
    fn static_element_mounts_and_builds_one_frame() {
        let prefs = OsPreferences::fallback();
        let mut prepared = App::prepare(Config::new().window(test_window()), &prefs, |_cx| {
            hello_box("hello")
        })
        .expect("valid config prepares");
        assert_eq!(prepared.resolved.material.requested(), Material::Glass);
        assert!(!prepared.resolved.material.reason().is_empty());
        let observed = observed_for(&prepared.resolved);
        let mut scene = Scene::new();
        prepared
            .ui
            .build_scene_into(&mut scene, &observed)
            .expect("one frame builds");
        assert!(
            scene.ordered().count() > 0,
            "static content emits scene ops"
        );
        assert_eq!(prepared.ui.build_error(), None);
        assert!(texts(&prepared.ui).iter().any(|t| t == "hello"));
        prepared.ui.dispose();
        assert_eq!(prepared.ui.text_run_count(), 0);
    }

    #[test]
    fn reactive_view_mounts_and_updates_on_explicit_deps() {
        let prefs = OsPreferences::fallback();
        let slot: Rc<RefCell<Option<WriteSignal<bool>>>> = Rc::new(RefCell::new(None));
        let slot_setup = Rc::clone(&slot);
        let mut prepared = App::prepare(Config::new().window(test_window()), &prefs, move |cx| {
            let (flag_r, flag_w): (ReadSignal<bool>, WriteSignal<bool>) = cx.scope().signal(false);
            *slot_setup.borrow_mut() = Some(flag_w);
            cx.view((flag_r,), move |(flag,)| {
                hello_box(if flag { "flag-on" } else { "flag-off" })
            })
        })
        .expect("reactive config prepares");
        let observed = observed_for(&prepared.resolved);
        let mut scene = Scene::new();
        prepared
            .ui
            .build_scene_into(&mut scene, &observed)
            .expect("initial frame builds");
        assert!(texts(&prepared.ui).iter().any(|t| t == "flag-off"));
        // Dependency write flows through the same sync + frame seam.
        slot.borrow()
            .as_ref()
            .expect("setup stashed the writer")
            .set(true);
        prepared
            .ui
            .sync()
            .expect("sync drains the structural render");
        prepared
            .ui
            .build_scene_into(&mut scene, &observed)
            .expect("updated frame builds");
        let labels = texts(&prepared.ui);
        assert!(
            labels.iter().any(|t| t == "flag-on"),
            "dep change rerenders: {labels:?}"
        );
        assert!(!labels.iter().any(|t| t == "flag-off"));
        assert_eq!(prepared.ui.build_error(), None);
        prepared.ui.dispose();
    }

    #[test]
    fn resolved_material_reaches_setup_for_ui_branching() {
        // Opaque policy forces solid even when glass is requested:
        // platform-independent, no capability polling in UI code.
        let mut prefs = OsPreferences::fallback();
        prefs.high_contrast = PrefState::On;
        let mut prepared = App::prepare(Config::new().window(test_window()), &prefs, |cx| {
            hello_box(if cx.material().is_glass() {
                "glass-ui"
            } else {
                "solid-ui"
            })
        })
        .expect("config prepares under opaque prefs");
        assert_eq!(prepared.resolved.material.requested(), Material::Glass);
        assert_eq!(prepared.resolved.material.resolved(), WindowMaterial::None);
        assert!(!prepared.resolved.material.is_glass());
        let observed = observed_for(&prepared.resolved);
        let mut scene = Scene::new();
        prepared
            .ui
            .build_scene_into(&mut scene, &observed)
            .expect("frame builds");
        assert!(texts(&prepared.ui).iter().any(|t| t == "solid-ui"));
        prepared.ui.dispose();
    }

    #[test]
    fn authoring_reduced_motion_snaps_tweens_with_os_prefs_off() {
        use aqua_ui::motion::{Easing, MotionKind, MotionValue, ReducedMotion};
        // OS reports no reduced-motion preference here.
        let prefs = OsPreferences::fallback();
        assert!(!prefs.reduce_motion.is_on());
        let tween = || MotionKind::Tween {
            duration_secs: 1.0,
            easing: Easing::Linear,
        };
        // Authoring request on: the host controller is in Reduce mode and a
        // real 1s tween snaps to its target on the first sample.
        let mut reduced = App::prepare(
            Config::new().reduced_motion(true).window(test_window()),
            &prefs,
            |_cx| hello_box("motion"),
        )
        .expect("config prepares");
        assert_eq!(
            reduced.ui.host().motion().reduced_motion(),
            ReducedMotion::Reduce
        );
        let motion = reduced.ui.host_mut().motion_mut();
        let h = motion.spawn(None, MotionValue::Scalar(0.0), tween(), 0.0);
        motion
            .set_target(h, MotionValue::Scalar(100.0), 0.0)
            .expect("live handle retargets");
        motion.advance(0.0);
        assert!(motion.is_settled(h), "tween snaps under Reduce");
        assert_eq!(motion.peek(h), Some(MotionValue::Scalar(100.0)));
        // Control: no request, OS off — full motion, tween still in flight.
        let mut full = App::prepare(Config::new().window(test_window()), &prefs, |_cx| {
            hello_box("motion")
        })
        .expect("config prepares");
        assert_eq!(
            full.ui.host().motion().reduced_motion(),
            ReducedMotion::Full
        );
        let motion = full.ui.host_mut().motion_mut();
        let h = motion.spawn(None, MotionValue::Scalar(0.0), tween(), 0.0);
        motion
            .set_target(h, MotionValue::Scalar(100.0), 0.0)
            .expect("live handle retargets");
        motion.advance(0.0);
        assert!(!motion.is_settled(h), "tween animates under Full");
        assert_eq!(motion.peek(h), Some(MotionValue::Scalar(0.0)));
        reduced.ui.dispose();
        full.ui.dispose();
    }

    #[test]
    fn dispose_runs_scope_cleanups_and_releases_text_runs() {
        let prefs = OsPreferences::fallback();
        let cleanups = Rc::new(Cell::new(0u32));
        let cleanups_setup = Rc::clone(&cleanups);
        let mut prepared = App::prepare(Config::new().window(test_window()), &prefs, move |cx| {
            let cleanups = Rc::clone(&cleanups_setup);
            cx.scope().on_cleanup(move || {
                cleanups.set(cleanups.get() + 1);
            });
            cx.view((cx.scope().signal(0u32).0,), move |_| hello_box("bye"))
        })
        .expect("config prepares");
        let observed = observed_for(&prepared.resolved);
        prepared
            .ui
            .build_scene_into(&mut Scene::new(), &observed)
            .expect("frame builds and mints runs");
        assert!(
            prepared.ui.text_run_count() > 0,
            "shaped text is retained before dispose"
        );
        prepared.ui.dispose();
        assert_eq!(cleanups.get(), 1, "root scope cleanup ran exactly once");
        assert_eq!(prepared.ui.text_run_count(), 0, "runs left the host");
        prepared.ui.dispose();
        assert_eq!(cleanups.get(), 1, "repeat dispose is a no-op");
    }

    #[test]
    fn invalid_config_rejects_before_setup_runs() {
        let prefs = OsPreferences::fallback();
        for config in [
            Config::new().window(Window::new().size(px(0.0), px(560.0))),
            Config::new().window(Window::new().size(px(760.0), px(f32::NAN))),
            Config::new().window(
                Window::new()
                    .size(px(100.0), px(100.0))
                    .min_size(px(200.0), px(50.0)),
            ),
            Config::new().fps(0),
            Config::new().window(Window::new().size(pct(50.0), px(560.0))),
        ] {
            let ran = Rc::new(Cell::new(false));
            let ran_setup = Rc::clone(&ran);
            let result = App::prepare(config, &prefs, move |_cx| {
                ran_setup.set(true);
                hello_box("must not mount")
            });
            assert!(
                matches!(result, Err(Error::Config(_))),
                "invalid config maps to Error::Config"
            );
            assert!(!ran.get(), "setup factory never executes on invalid config");
        }
    }

    #[test]
    fn setup_panic_disposes_host_and_maps_to_init() {
        let prefs = OsPreferences::fallback();
        let cleanups = Rc::new(Cell::new(0u32));
        let cleanups_setup = Rc::clone(&cleanups);
        let result: Result<Prepared, Error> = App::prepare(
            Config::new().window(test_window()),
            &prefs,
            move |cx| -> Element {
                let cleanups = Rc::clone(&cleanups_setup);
                cx.scope().on_cleanup(move || {
                    cleanups.set(cleanups.get() + 1);
                });
                panic!("boom in setup");
            },
        );
        assert!(
            matches!(result, Err(Error::Init(_))),
            "setup panic maps to Error::Init"
        );
        assert_eq!(cleanups.get(), 1, "host disposed on setup failure");
    }

    #[test]
    fn headless_run_builds_one_frame_black_box() {
        // True black-box `.run(...)`: no window, no GPU, Ok on success.
        App::new()
            .config(Config::new().headless(true).window(test_window()))
            .run(|_cx| hello_box("headless"))
            .expect("headless one-frame run succeeds");
    }

    #[test]
    fn headless_run_rejects_invalid_config_without_running_setup() {
        let ran = Rc::new(Cell::new(false));
        let ran_setup = Rc::clone(&ran);
        let result = App::new()
            .config(
                Config::new()
                    .headless(true)
                    .window(Window::new().size(px(0.0), px(0.0))),
            )
            .run(move |_cx| {
                ran_setup.set(true);
                hello_box("must not mount")
            });
        assert!(matches!(result, Err(Error::Config(_))));
        assert!(!ran.get());
    }
}
