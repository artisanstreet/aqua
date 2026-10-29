//! Verification: canonical `App` builder end-to-end (black-box).
//!
//! Covers the frozen `App`/`Config`/`Window`/`Theme`/`Material`/`Context`/
//! `IntoView` seam against the public facade surface only:
//! - headless single `run` executes setup signals/effects and disposes the
//!   scope (cleanup runs), via the canonical one-frame lifecycle
//!   (mount -> flush/layout/scene -> dispose), not a fake test helper;
//! - static `run` (plain `Element`) and reactive `run`
//!   (`cx.view(explicit deps, render)`) both succeed headless;
//! - invalid builder values are stored until resolve: `run` returns
//!   `Err(aqua::Error)` without invoking the factory;
//! - pure `Config::resolve` fallback semantics: Glass stands under unknown
//!   prefs, downgrades to solid under high-contrast / transparency-off;
//! - no `App` (builder) vs `Application` (low-level trait) name collision.
//!
//! Assumptions (frozen names per `docs/APP_BUILDER_CONTRACT.md`; compile
//! once the App/Config owners land):
//! - `aqua::config::{Config, Window, Theme, Material}` builders;
//!   `Config::new().window(...).headless(true)` mirrors the old
//!   `AquaConfig.headless` flag in builder form.
//! - `aqua::{App, Context, IntoView}` at the crate root;
//!   `App::new().config(..).run(setup)` with
//!   `setup: FnOnce(&Context) -> V, V: IntoView`.
//! - `Context::scope() -> &Scope`, `material() -> &ResolvedMaterial`,
//!   `view(deps, render) -> ReactiveView<D, F>`;
//!   `ResolvedMaterial::is_glass() -> bool`.
//! - `config.resolve(&prefs) -> Result<ResolvedConfig, ConfigError>` with
//!   `ResolvedConfig { runtime: AquaConfig, material: ResolvedMaterial }`;
//!   `aqua::Error` wraps `ConfigError` and `From<String>` holds for the CLI
//!   `map_err(Into::into)` path.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aqua::config::{Config, Material, Theme, Titlebar, Window};
use aqua::{App, Application, Context, IntoView};
use aqua_platform::prefs::{OsPreferences, PrefState};

fn window() -> Window {
    Window::new()
        .title("builder-test")
        .size(aqua::px(760.0), aqua::px(560.0))
        .min_size(aqua::px(480.0), aqua::px(360.0))
        .theme(Theme::Dark)
        .material(Material::Glass)
        .titlebar(Titlebar::Overlay)
}

fn headless_config() -> Config {
    Config::new().window(window()).headless(true)
}

#[test]
fn static_headless_run_mounts_and_disposes_scope() {
    use aqua::ui::Text as UiText;
    let factory_ran = Arc::new(AtomicUsize::new(0));
    let cleaned = Arc::new(AtomicBool::new(false));
    let factory_probe = factory_ran.clone();
    let cleaned_probe = cleaned.clone();
    let result = App::new().config(headless_config()).run(|cx: &Context| {
        factory_probe.fetch_add(1, Ordering::SeqCst);
        let flag = cleaned_probe.clone();
        cx.scope()
            .on_cleanup(move || flag.store(true, Ordering::SeqCst));
        UiText::builder().arg("hello").build()
    });
    assert!(result.is_ok(), "headless static run succeeds: {result:?}");
    assert_eq!(factory_ran.load(Ordering::SeqCst), 1, "factory runs once");
    assert!(
        cleaned.load(Ordering::SeqCst),
        "headless lifecycle disposes the setup scope"
    );
}

#[test]
fn reactive_headless_run_executes_explicit_deps() {
    use aqua::ui::Text as UiText;
    let renders = Arc::new(AtomicUsize::new(0));
    let renders_probe = renders.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_probe = seen.clone();
    let result = App::new().config(headless_config()).run(|cx: &Context| {
        let (value_r, value_w) = cx.scope().signal(41i32);
        value_w.set(42);
        let deps = (value_r,);
        cx.view(deps, move |(value,)| {
            renders_probe.fetch_add(1, Ordering::SeqCst);
            seen_probe.lock().unwrap().push(value);
            UiText::builder().arg(format!("{value}")).build()
        })
    });
    assert!(result.is_ok(), "headless reactive run succeeds: {result:?}");
    assert!(
        renders.load(Ordering::SeqCst) >= 1,
        "explicit-dep render executes at least once"
    );
    assert!(
        seen.lock().unwrap().contains(&42),
        "setup signal effect visible to render: {:?}",
        seen.lock().unwrap()
    );
}

#[test]
fn invalid_config_errors_without_invoking_factory() {
    use aqua::ui::Text as UiText;
    // Zero width is stored (not panicking in the setter) and rejected at
    // resolve; the factory must never run.
    let bad = Config::new().window(
        Window::new()
            .title("bad")
            .size(aqua::px(0.0), aqua::px(560.0))
            .min_size(aqua::px(480.0), aqua::px(360.0))
            .theme(Theme::Dark)
            .material(Material::Glass),
    );
    let invoked = Arc::new(AtomicBool::new(false));
    let probe = invoked.clone();
    let result = App::new().config(bad).run(|_: &Context| {
        probe.store(true, Ordering::SeqCst);
        UiText::builder().arg("must not mount").build()
    });
    assert!(result.is_err(), "invalid config propagates Err, not a run");
    assert!(
        !invoked.load(Ordering::SeqCst),
        "factory never invoked on resolve failure"
    );
}

#[test]
fn pure_resolution_glass_stands_opaque_prefs_fall_back() {
    let config = Config::new().window(window());
    // Unknown prefs never force the fallback by themselves: the outcome
    // follows the real platform adapter (glass on capable Windows, solid
    // elsewhere) — never fabricated. The request is always remembered.
    let resolved = config
        .resolve(&OsPreferences::fallback())
        .expect("valid config resolves");
    assert_eq!(resolved.material.requested(), Material::Glass);
    assert_eq!(
        resolved.runtime.window.titlebar,
        Titlebar::Overlay,
        "overlay titlebar passes through resolve"
    );
    let capable =
        aqua_platform::material::material_capability(aqua_platform::WindowMaterial::Acrylic)
            .supported;
    assert_eq!(
        resolved.material.is_glass(),
        capable,
        "fallback follows the real adapter capability"
    );
    assert!(!resolved.material.reason().is_empty());
    // High contrast forces the solid fallback.
    let mut high_contrast = OsPreferences::fallback();
    high_contrast.high_contrast = PrefState::On;
    let downgraded = config
        .resolve(&high_contrast)
        .expect("valid config resolves");
    assert!(
        !downgraded.material.is_glass(),
        "high-contrast downgrades to solid"
    );
    // Transparency-off forces the same honest fallback.
    let mut no_transparency = OsPreferences::fallback();
    no_transparency.reduce_transparency = PrefState::On;
    let downgraded = config
        .resolve(&no_transparency)
        .expect("valid config resolves");
    assert!(
        !downgraded.material.is_glass(),
        "transparency-off downgrades to solid"
    );
    // Resolved config still carries a runtime config for the lower level.
    let _runtime: &aqua::AquaConfig = &resolved.runtime;
}

#[test]
fn app_builder_and_application_trait_coexist() {
    use aqua::ui::Text as UiText;
    // Both frozen names import from the same facade root without collision.
    fn builds_app() -> App {
        App::new()
    }
    struct Probe;
    impl Application for Probe {
        fn on_window_event(
            &mut self,
            _ctx: &mut aqua::AppCtx,
            _id: aqua_platform::AquaWindowId,
            _event: &aqua_platform::WindowEvent,
        ) {
        }
        fn on_input(
            &mut self,
            _ctx: &mut aqua::AppCtx,
            _id: aqua_platform::AquaWindowId,
            _event: &aqua_core::InputEvent,
        ) {
        }
        fn build_scene(
            &mut self,
            _window: &aqua_platform::ObservedWindowState,
            _now: aqua::Instant,
        ) -> Option<aqua::Instant> {
            None
        }
    }
    let _builder = builds_app();
    let _probe = Probe;
    // `IntoView` is sealed over `Element` (static) — compile proof that a
    // plain element satisfies the `run` setup return bound.
    fn accepts_into_view<V: IntoView>(_: V) {}
    accepts_into_view(UiText::builder().arg("sealed").build());
}
