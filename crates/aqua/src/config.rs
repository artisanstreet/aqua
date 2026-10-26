//! Facade-level runtime configuration.
//!
//! This module composes peer-owned option types; it defines no competing
//! windowing, pacing, or presentation semantics. Frame pacing config flows
//! straight into [`aqua_platform::scheduler::Scheduler`]
//! (`set_fps_target(Option<NonZeroU32>)`, `None` = unpaced demand-only).

use std::num::NonZeroU32;

use aqua_platform::material::material_capability;
use aqua_platform::prefs::{prefers_opaque_frame, OsPreferences};
use aqua_platform::window::ThemePreference;
pub use aqua_platform::window::{Titlebar, WindowOptions};
use aqua_platform::WindowMaterial;
use aqua_ui::style::Length;

/// Top-level application configuration.
#[derive(Clone, Debug)]
pub struct AquaConfig {
    /// Window creation options (platform-owned type, re-exported).
    pub window: WindowOptions,
    /// Frame-production target, passed unchanged to the platform scheduler.
    /// `None` (default) = unpaced: produce on demand, never tick while idle.
    /// Any non-zero value is accepted; throughput stays bounded by vsync,
    /// presentation policy, and backpressure.
    pub fps_target: Option<NonZeroU32>,
    /// `true` = vsync-paced presentation (`Fifo`); `false` = immediate
    /// (`Immediate`, tearing allowed where the backend supports it, otherwise
    /// the backend falls back and the runtime reports it).
    pub vsync: bool,
    /// Native desktop material behind the window (Mica/Acrylic on Windows 11;
    /// honest fallback elsewhere). Non-`None` requires a transparent window +
    /// alpha surface: the runtime forces `window.transparent` (loudly) and
    /// downgrades to `None` under high-contrast/transparency-off prefs.
    pub material: aqua_platform::WindowMaterial,
    /// Headless mode: no window or GPU. Drives the app pump in tests.
    pub headless: bool,
    /// When true, decorative motion snaps instead of animating (also set
    /// from OS prefs at startup — see `GETTING_STARTED.md`).
    pub reduced_motion: bool,
}

impl Default for AquaConfig {
    fn default() -> Self {
        Self {
            window: WindowOptions::default(),
            fps_target: None,
            vsync: true,
            material: aqua_platform::WindowMaterial::None,
            headless: false,
            reduced_motion: false,
        }
    }
}

impl AquaConfig {
    /// Validate window options early (empty sizes rejected before any
    /// window exists). Returns the platform error unchanged.
    pub fn validated(&self) -> Result<(), aqua_platform::window::WindowError> {
        self.window.validated()
    }

    /// Resolve the effective material against OS preferences: high contrast
    /// or transparency-off forces the solid fallback (Mica guidance).
    /// Returns the downgraded value when policy overrides the request.
    pub fn effective_material(
        &self,
        prefs: &aqua_platform::prefs::OsPreferences,
    ) -> aqua_platform::WindowMaterial {
        use aqua_platform::WindowMaterial as M;
        if self.material != M::None && aqua_platform::prefs::prefers_opaque_frame(prefs) {
            eprintln!("aqua: high-contrast/transparency-off — downgrading material to opaque");
            return M::None;
        }
        self.material
    }

    /// Convenience: fixed-rate pacing (e.g. `Some(60)`), or `None` unpaced.
    /// Zero is rejected; the previous value is kept on error.
    pub fn set_fps(&mut self, fps: u32) -> Result<(), &'static str> {
        if fps == 0 {
            return Err("frame-rate target must be non-zero");
        }
        self.fps_target = NonZeroU32::new(fps);
        Ok(())
    }

    /// Apply the pacing target to a platform scheduler. The single pacing
    /// owner is the scheduler; this config holds no duplicate gate state.
    pub fn apply_to_scheduler(&self, scheduler: &mut aqua_platform::Scheduler) {
        scheduler.set_fps_target(self.fps_target);
    }
}

/// Authoring theme: semantic intent, resolved to the platform
/// [`ThemePreference`] at [`Config::resolve`] time. `System` (default)
/// follows the OS (`Unknown` until the OS reports one — never guessed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// Follow the OS theme.
    #[default]
    System,
    /// Force the light chrome variant.
    Light,
    /// Force the dark chrome variant.
    Dark,
}

impl Theme {
    fn as_platform(self) -> ThemePreference {
        match self {
            Theme::System => ThemePreference::Unknown,
            Theme::Light => ThemePreference::Light,
            Theme::Dark => ThemePreference::Dark,
        }
    }
}

/// Authoring material: semantic intent owned by [`Window`]. `Solid`
/// (default) is the opaque frame; `Glass` requests the platform glass
/// candidate. The Glass -> Windows Acrylic mapping lives in
/// [`Config::resolve`] (via the `aqua-platform` capability + prefs
/// adapters) — never in the caller, and no macOS/Linux blur backends are
/// invented: off-Windows `Glass` resolves to `Solid` with an honest reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Material {
    /// Opaque frame. Always available.
    #[default]
    Solid,
    /// Desktop glass where the platform supports it (Windows Acrylic),
    /// solid fallback elsewhere.
    Glass,
}

/// Authoring window builder: all fields private, setters consume and return
/// `Self` for chaining. Setters never panic and never touch the OS/GPU —
/// invalid values are stored and reported by [`Config::resolve`].
#[derive(Debug, Clone)]
pub struct Window {
    title: String,
    size: (Length, Length),
    min_size: Option<(Length, Length)>,
    max_size: Option<(Length, Length)>,
    resizable: bool,
    decorations: bool,
    transparent: bool,
    theme: Theme,
    material: Material,
    /// Authoring titlebar mode (platform-owned `Titlebar`; re-exported,
    /// never duplicated here). Default `Native`.
    titlebar: Titlebar,
    /// Custom titlebar height in logical px (`px()` only; validated at
    /// resolve). Default `px(40.0)`. Only consumed by overlay/custom
    /// titlebar paths; validated regardless so invalid values never hide
    /// behind a mode that ignores them.
    titlebar_height: Length,
    /// Caption-button presence hints. Passed through to the platform;
    /// moot when the window ends up undecorated.
    minimizable: bool,
    maximizable: bool,
    closable: bool,
}

impl Window {
    /// Defaults mirror [`WindowOptions::default`]: 800x600, resizable,
    /// decorated, opaque, system theme, solid material, native titlebar
    /// with 40 logical px height and all caption buttons present.
    #[must_use]
    pub fn new() -> Self {
        Self {
            title: WindowOptions::default().title,
            size: (aqua_ui::style::px(800.0), aqua_ui::style::px(600.0)),
            min_size: None,
            max_size: None,
            resizable: true,
            decorations: true,
            transparent: false,
            theme: Theme::System,
            material: Material::Solid,
            titlebar: Titlebar::Native,
            titlebar_height: aqua_ui::style::px(40.0),
            minimizable: true,
            maximizable: true,
            closable: true,
        }
    }

    /// Window title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Initial inner size in logical px (`px()` only; validated at resolve).
    /// Last call wins.
    #[must_use]
    pub fn size(mut self, width: Length, height: Length) -> Self {
        self.size = (width, height);
        self
    }

    /// Minimum inner size in logical px (`px()` only; validated at resolve).
    /// Last call wins.
    #[must_use]
    pub fn min_size(mut self, width: Length, height: Length) -> Self {
        self.min_size = Some((width, height));
        self
    }

    /// Clear a previously set minimum size.
    #[must_use]
    pub fn clear_min_size(mut self) -> Self {
        self.min_size = None;
        self
    }

    /// Maximum inner size in logical px (`px()` only; validated at resolve).
    /// Last call wins.
    #[must_use]
    pub fn max_size(mut self, width: Length, height: Length) -> Self {
        self.max_size = Some((width, height));
        self
    }

    /// Clear a previously set maximum size.
    #[must_use]
    pub fn clear_max_size(mut self) -> Self {
        self.max_size = None;
        self
    }

    /// Whether the window is user-resizable. Last call wins.
    #[must_use]
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Whether the window has OS decorations. Last call wins.
    #[must_use]
    pub fn decorations(mut self, decorations: bool) -> Self {
        self.decorations = decorations;
        self
    }

    /// Request a transparent framebuffer. Resolve additionally forces this
    /// on whenever the resolved material needs it (mirrors the runtime's
    /// loud transparent upgrade); a resolved solid material leaves the
    /// explicit value untouched. Last call wins.
    #[must_use]
    pub fn transparent(mut self, transparent: bool) -> Self {
        self.transparent = transparent;
        self
    }

    /// Authoring theme. Last call wins.
    #[must_use]
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Authoring material. Last call wins.
    #[must_use]
    pub fn material(mut self, material: Material) -> Self {
        self.material = material;
        self
    }

    /// Titlebar mode: `Native` (default, no change), `Overlay` (native
    /// caption buttons over full client content), or `Hidden` (no caption,
    /// lowers to undecorated). Layering with `decorations` is resolved in
    /// [`Config::resolve`]: legacy `decorations(false)` stays fully
    /// frameless and wins over any mode. Last call wins.
    #[must_use]
    pub fn titlebar(mut self, titlebar: Titlebar) -> Self {
        self.titlebar = titlebar;
        self
    }

    /// Custom titlebar height in logical px (`px()` only; finite positive,
    /// validated at resolve like all window sizes). Only consumed by
    /// overlay/custom titlebar paths, but always validated so an invalid
    /// value cannot hide behind `Native`. Last call wins.
    #[must_use]
    pub fn titlebar_height(mut self, height: Length) -> Self {
        self.titlebar_height = height;
        self
    }

    /// Whether the minimize caption button is present. Passed through to
    /// the platform; moot when the window ends up undecorated. Last call wins.
    #[must_use]
    pub fn minimizable(mut self, minimizable: bool) -> Self {
        self.minimizable = minimizable;
        self
    }

    /// Whether the maximize caption button is present. Passed through to
    /// the platform; moot when the window ends up undecorated. Last call wins.
    #[must_use]
    pub fn maximizable(mut self, maximizable: bool) -> Self {
        self.maximizable = maximizable;
        self
    }

    /// Whether the close caption button is present. Passed through to
    /// the platform; moot when the window ends up undecorated. Last call wins.
    #[must_use]
    pub fn closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }
}

impl Default for Window {
    fn default() -> Self {
        Self::new()
    }
}

/// Authoring app configuration builder: all fields private, setters consume
/// and return `Self`. Setters never panic — invalid values are stored and
/// reported by [`resolve`](Config::resolve). Lower-level [`AquaConfig`]
/// keeps its public fields for advanced/runtime use; this builder is the
/// canonical authoring path.
#[derive(Debug, Clone)]
pub struct Config {
    window: Window,
    /// `None` = unpaced (default). `Some(0)` is storable but invalid:
    /// resolve rejects it instead of the setter panicking.
    fps: Option<u32>,
    vsync: bool,
    headless: bool,
    reduced_motion: bool,
}

impl Config {
    /// Defaults mirror [`AquaConfig::default`]: default window, unpaced,
    /// vsync on, windowed, full motion.
    #[must_use]
    pub fn new() -> Self {
        Self {
            window: Window::new(),
            fps: None,
            vsync: true,
            headless: false,
            reduced_motion: false,
        }
    }

    /// Authoring window. Replaces the previous value wholesale.
    #[must_use]
    pub fn window(mut self, window: Window) -> Self {
        self.window = window;
        self
    }

    /// Fixed frame-production rate. Stored verbatim (including `0`);
    /// [`resolve`](Config::resolve) rejects zero with
    /// [`ConfigError::InvalidFps`]. Last call wins.
    #[must_use]
    pub fn fps(mut self, fps: u32) -> Self {
        self.fps = Some(fps);
        self
    }

    /// Unpaced production (default): produce on demand, never tick idle.
    #[must_use]
    pub fn unpaced(mut self) -> Self {
        self.fps = None;
        self
    }

    /// Vsync-paced presentation (`true`, default) vs immediate. Last call wins.
    #[must_use]
    pub fn vsync(mut self, vsync: bool) -> Self {
        self.vsync = vsync;
        self
    }

    /// Headless mode: no window or GPU. Last call wins.
    #[must_use]
    pub fn headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

    /// Request reduced decorative motion. Last call wins.
    #[must_use]
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = reduced;
        self
    }

    /// Validate the stored builder values against `prefs` and lower them to
    /// the runtime [`AquaConfig`] plus the startup [`ResolvedMaterial`].
    /// Pure: no OS/GPU initialization. All setter-stored values (including
    /// invalid ones) are checked here; the first failure is returned.
    pub fn resolve(&self, prefs: &OsPreferences) -> Result<ResolvedConfig, ConfigError> {
        let inner = px_pair("size", self.window.size)?;
        let min = self
            .window
            .min_size
            .map(|pair| px_pair("min_size", pair))
            .transpose()?;
        let max = self
            .window
            .max_size
            .map(|pair| px_pair("max_size", pair))
            .transpose()?;
        if let (Some(lo), Some(hi)) = (min, max) {
            if lo.width > hi.width || lo.height > hi.height {
                return Err(ConfigError::BoundsOrder {
                    what: "min_size must not exceed max_size per axis",
                });
            }
        }
        // Initial-size constraint behavior (explicit): the requested inner
        // size must already sit inside [min, max]; resolve errors rather
        // than silently clamping or resizing.
        if let Some(lo) = min {
            if inner.width < lo.width || inner.height < lo.height {
                return Err(ConfigError::InitialOutOfBounds {
                    what: "size must not be smaller than min_size per axis",
                });
            }
        }
        if let Some(hi) = max {
            if inner.width > hi.width || inner.height > hi.height {
                return Err(ConfigError::InitialOutOfBounds {
                    what: "size must not be larger than max_size per axis",
                });
            }
        }
        let fps_target = match self.fps {
            None => None,
            Some(0) => return Err(ConfigError::InvalidFps { fps: 0 }),
            Some(fps) => NonZeroU32::new(fps),
        };
        let (active, reason) = resolve_material(self.window.material, prefs);
        let transparent = self.window.transparent
            || aqua_platform::material::material_needs_transparent_window(active);
        // Titlebar layering (explicit, no silent coercion beyond this):
        // - legacy `decorations(false)` stays fully frameless and wins over
        //   any titlebar mode (coerced to `Hidden` for the lowered options);
        // - explicit `Hidden` lowers to undecorated;
        // - `Overlay` keeps native caption buttons with full client content
        //   (Windows-only behavior applied by the runtime; elsewhere the
        //   runtime falls back to `Native`);
        // - `Native` (default) passes through unchanged.
        let titlebar = if self.window.decorations {
            self.window.titlebar
        } else {
            Titlebar::Hidden
        };
        let decorations = self.window.decorations && titlebar != Titlebar::Hidden;
        let titlebar_height = px_axis("titlebar_height", self.window.titlebar_height)?;
        let runtime = AquaConfig {
            window: WindowOptions {
                title: self.window.title.clone(),
                inner_size: inner,
                min_size: min,
                max_size: max,
                resizable: self.window.resizable,
                decorations,
                transparent,
                theme: self.window.theme.as_platform(),
                titlebar,
                titlebar_height,
                minimizable: self.window.minimizable,
                maximizable: self.window.maximizable,
                closable: self.window.closable,
            },
            fps_target,
            vsync: self.vsync,
            material: active,
            headless: self.headless,
            reduced_motion: self.reduced_motion,
        };
        Ok(ResolvedConfig {
            runtime,
            material: ResolvedMaterial {
                requested: self.window.material,
                resolved: active,
                reason,
            },
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

/// Lowered configuration: the runtime [`AquaConfig`] the app worker feeds
/// to the event loop, plus the startup [`ResolvedMaterial`].
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// Runtime configuration (advanced APIs keep using this directly).
    pub runtime: AquaConfig,
    /// Startup material policy/capability resolution.
    pub material: ResolvedMaterial,
}

/// Startup material resolution: the authoring request plus the resolved
/// platform candidate and the honest reason. This does NOT claim the native
/// material has been applied — the window does not exist yet; the adapter
/// applies the candidate at window creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedMaterial {
    requested: Material,
    resolved: WindowMaterial,
    reason: &'static str,
}

impl ResolvedMaterial {
    /// The authoring request (`Solid`/`Glass`).
    #[must_use]
    pub fn requested(&self) -> Material {
        self.requested
    }

    /// The resolved platform candidate (`None` = solid frame,
    /// `Acrylic` = Windows glass candidate). Apply-time, not applied.
    #[must_use]
    pub fn resolved(&self) -> WindowMaterial {
        self.resolved
    }

    /// Why this candidate was selected (policy or capability line).
    #[must_use]
    pub fn reason(&self) -> &'static str {
        self.reason
    }

    /// Whether startup resolution selected a glass candidate. Backs UI
    /// fallback styling (tints Legible over glass vs solid). True means a
    /// glass candidate was *selected* under current prefs/capability — not
    /// that the compositor has applied anything pre-window.
    #[must_use]
    pub fn is_glass(&self) -> bool {
        self.resolved != WindowMaterial::None
    }
}

/// Builder validation failures. Returned by [`Config::resolve`]; the app
/// worker's `Error` wraps this type. Setters never produce these eagerly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A window size is not a finite positive `px()` value (covers zero,
    /// negative, NaN, infinite, and `Pct`/`Auto` which have no referent at
    /// the windowing boundary).
    InvalidSize {
        /// Which size field failed (`"size"`, `"min_size"`, `"max_size"`).
        field: &'static str,
        /// Why the value is unusable.
        reason: &'static str,
    },
    /// `min_size` exceeds `max_size` on at least one axis.
    BoundsOrder {
        /// Ordering rule that was violated.
        what: &'static str,
    },
    /// The initial `size` sits outside the `[min_size, max_size]` range.
    /// Resolve errors explicitly instead of clamping.
    InitialOutOfBounds {
        /// Which bound the initial size violates.
        what: &'static str,
    },
    /// Frame-rate target must be nonzero (`None` = unpaced instead).
    InvalidFps {
        /// The rejected value (always `0`).
        fps: u32,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::InvalidSize { field, reason } => {
                write!(f, "invalid window {field}: {reason}")
            }
            ConfigError::BoundsOrder { what } => write!(f, "invalid window bounds: {what}"),
            ConfigError::InitialOutOfBounds { what } => {
                write!(f, "initial window size out of bounds: {what}")
            }
            ConfigError::InvalidFps { fps } => {
                write!(f, "frame-rate target must be non-zero (got {fps})")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Convert one authoring `(Length, Length)` pair to a platform
/// [`LogicalSize`](aqua_platform::geometry::LogicalSize): `px()` only,
/// finite, strictly positive.
fn px_pair(
    field: &'static str,
    (w, h): (Length, Length),
) -> Result<aqua_platform::geometry::LogicalSize, ConfigError> {
    Ok(aqua_platform::geometry::LogicalSize::new(
        px_axis(field, w)?,
        px_axis(field, h)?,
    ))
}

fn px_axis(field: &'static str, len: Length) -> Result<f64, ConfigError> {
    match len {
        Length::Px(v) => {
            let d = f64::from(v);
            if !d.is_finite() {
                return Err(ConfigError::InvalidSize {
                    field,
                    reason: "must be finite",
                });
            }
            if d <= 0.0 {
                return Err(ConfigError::InvalidSize {
                    field,
                    reason: "must be positive",
                });
            }
            Ok(d)
        }
        Length::Pct(_) => Err(ConfigError::InvalidSize {
            field,
            reason: "window sizes take px() only; percent has no referent here",
        }),
        Length::Auto => Err(ConfigError::InvalidSize {
            field,
            reason: "window sizes take px() only; auto has no meaning here",
        }),
    }
}

/// Startup material policy: authoring `Glass` resolves to the platform
/// Acrylic candidate only when prefs permit composited frames AND the
/// compiling platform reports a real capability; otherwise it falls back
/// to solid with the adapter's honest reason. `Solid` always resolves to
/// the opaque frame. No macOS/Linux blur backends are invented.
fn resolve_material(requested: Material, prefs: &OsPreferences) -> (WindowMaterial, &'static str) {
    match requested {
        Material::Solid => (WindowMaterial::None, "explicit solid frame"),
        Material::Glass => {
            if prefers_opaque_frame(prefs) {
                return (
                    WindowMaterial::None,
                    "high-contrast/transparency-off: solid fallback per Mica guidance",
                );
            }
            let cap = material_capability(WindowMaterial::Acrylic);
            if cap.supported {
                (cap.active, cap.reason)
            } else {
                (WindowMaterial::None, cap.reason)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_zero_rejected_nonzero_passthrough() {
        let mut c = AquaConfig::default();
        assert_eq!(c.fps_target, None);
        assert!(c.set_fps(0).is_err());
        assert_eq!(c.fps_target, None); // unchanged
        c.set_fps(900).unwrap();
        assert_eq!(c.fps_target.unwrap().get(), 900);

        let mut s = aqua_platform::Scheduler::new();
        c.apply_to_scheduler(&mut s);
        assert_eq!(s.fps_target().unwrap().get(), 900);
    }

    #[test]
    fn material_downgraded_under_opaque_prefs() {
        use aqua_platform::prefs::{OsPreferences, PrefState};
        let c = AquaConfig {
            material: aqua_platform::WindowMaterial::Mica,
            ..Default::default()
        };
        // Unknown prefs: request stands.
        assert_eq!(
            c.effective_material(&OsPreferences::fallback()),
            aqua_platform::WindowMaterial::Mica
        );
        // High contrast: solid fallback.
        let mut p = OsPreferences::fallback();
        p.high_contrast = PrefState::On;
        assert_eq!(
            c.effective_material(&p),
            aqua_platform::WindowMaterial::None
        );
    }

    #[test]
    fn empty_window_size_rejected_before_window_exists() {
        let mut c = AquaConfig::default();
        c.window.inner_size = aqua_platform::geometry::LogicalSize::new(0.0, 600.0);
        assert!(c.validated().is_err());
    }

    // -- Authoring builder tests (pure supplied prefs; no OS/GPU init) --

    use aqua_platform::prefs::PrefState;
    use aqua_ui::style::{auto, pct, px};

    fn glass() -> Config {
        Config::new().window(Window::new().material(Material::Glass))
    }

    #[test]
    fn agreed_sample_resolves() {
        let cfg = Config::new().window(
            Window::new()
                .title("Aqua — Tasks")
                .size(px(760.0), px(560.0))
                .min_size(px(480.0), px(360.0))
                .theme(Theme::Dark)
                .material(Material::Glass),
        );
        let r = cfg.resolve(&OsPreferences::fallback()).unwrap();
        assert_eq!(r.runtime.window.title, "Aqua — Tasks");
        assert_eq!(
            r.runtime.window.inner_size,
            aqua_platform::geometry::LogicalSize::new(760.0, 560.0)
        );
        assert_eq!(
            r.runtime.window.min_size,
            Some(aqua_platform::geometry::LogicalSize::new(480.0, 360.0))
        );
        assert_eq!(r.runtime.window.theme, ThemePreference::Dark);
        // Request is remembered even where the platform cannot do glass.
        assert_eq!(r.material.requested(), Material::Glass);
        assert!(!r.material.reason().is_empty());
        // Expectation comes from the real adapter, never fabricated:
        // supported platforms select the Acrylic candidate, others fall
        // back to solid with the adapter's honest reason.
        let cap = material_capability(aqua_platform::WindowMaterial::Acrylic);
        assert_eq!(r.material.is_glass(), cap.supported);
        if cap.supported {
            assert_eq!(
                r.material.resolved(),
                aqua_platform::WindowMaterial::Acrylic
            );
            assert!(r.runtime.window.transparent);
        } else {
            assert_eq!(r.material.resolved(), aqua_platform::WindowMaterial::None);
            assert_eq!(r.material.reason(), cap.reason);
        }
    }

    #[test]
    fn defaults_match_sensible_runtime_defaults() {
        let r = Config::new().resolve(&OsPreferences::fallback()).unwrap();
        assert_eq!(r.runtime.window, WindowOptions::default());
        assert_eq!(r.runtime.fps_target, None);
        assert!(r.runtime.vsync);
        assert_eq!(r.runtime.material, aqua_platform::WindowMaterial::None);
        assert!(!r.runtime.headless);
        assert!(!r.runtime.reduced_motion);
        assert_eq!(r.material.requested(), Material::Solid);
        assert!(!r.material.is_glass());
    }

    #[test]
    fn repeated_setters_last_wins() {
        let r = Config::new()
            .window(
                Window::new()
                    .title("first")
                    .title("second")
                    .size(px(100.0), px(100.0))
                    .size(px(640.0), px(480.0))
                    .theme(Theme::Light)
                    .theme(Theme::Dark)
                    .material(Material::Glass)
                    .material(Material::Solid)
                    .resizable(false)
                    .resizable(true),
            )
            .fps(30)
            .fps(60)
            .vsync(false)
            .vsync(true)
            .resolve(&OsPreferences::fallback())
            .unwrap();
        assert_eq!(r.runtime.window.title, "second");
        assert_eq!(
            r.runtime.window.inner_size,
            aqua_platform::geometry::LogicalSize::new(640.0, 480.0)
        );
        assert_eq!(r.runtime.window.theme, ThemePreference::Dark);
        assert_eq!(r.material.requested(), Material::Solid);
        assert!(r.runtime.window.resizable);
        assert!(r.runtime.vsync);
        assert_eq!(r.runtime.fps_target.unwrap().get(), 60);
    }

    #[test]
    fn window_replaced_wholesale_and_runtime_options_compose() {
        let r = Config::new()
            .window(Window::new().title("first"))
            .window(Window::new().title("second").size(px(320.0), px(240.0)))
            .fps(120)
            .vsync(false)
            .headless(true)
            .reduced_motion(true)
            .resolve(&OsPreferences::fallback())
            .unwrap();
        assert_eq!(r.runtime.window.title, "second");
        assert_eq!(
            r.runtime.window.inner_size,
            aqua_platform::geometry::LogicalSize::new(320.0, 240.0)
        );
        assert_eq!(r.runtime.fps_target.unwrap().get(), 120);
        assert!(!r.runtime.vsync);
        assert!(r.runtime.headless);
        assert!(r.runtime.reduced_motion);
    }

    #[test]
    fn invalid_sizes_stored_until_resolve() {
        // Setters must not panic; resolve reports the first failure.
        for (w, h) in [
            (px(0.0), px(600.0)),
            (px(-1.0), px(600.0)),
            (px(f32::NAN), px(600.0)),
            (px(f32::INFINITY), px(600.0)),
            (px(800.0), px(f32::NEG_INFINITY)),
            (pct(50.0), px(600.0)),
            (auto(), px(600.0)),
            (px(800.0), pct(100.0)),
        ] {
            let cfg = Config::new().window(Window::new().size(w, h));
            assert!(
                matches!(
                    cfg.resolve(&OsPreferences::fallback()),
                    Err(ConfigError::InvalidSize { field: "size", .. })
                ),
                "size ({w:?}, {h:?}) must fail at resolve"
            );
        }
        // Fractional logical px are valid window sizes.
        let r = Config::new()
            .window(Window::new().size(px(760.5), px(560.25)))
            .resolve(&OsPreferences::fallback())
            .unwrap();
        assert_eq!(
            r.runtime.window.inner_size,
            aqua_platform::geometry::LogicalSize::new(760.5, 560.25)
        );
        // Limits validate too, with their field names.
        let bad_min = Config::new().window(Window::new().min_size(px(0.0), px(10.0)));
        assert!(matches!(
            bad_min.resolve(&OsPreferences::fallback()),
            Err(ConfigError::InvalidSize {
                field: "min_size",
                ..
            })
        ));
        let bad_max = Config::new().window(Window::new().max_size(px(10.0), auto()));
        assert!(matches!(
            bad_max.resolve(&OsPreferences::fallback()),
            Err(ConfigError::InvalidSize {
                field: "max_size",
                ..
            })
        ));
    }

    #[test]
    fn bounds_ordering_and_initial_constraints_explicit() {
        // min > max per axis is rejected.
        let cfg = Config::new().window(
            Window::new()
                .size(px(800.0), px(600.0))
                .min_size(px(900.0), px(100.0))
                .max_size(px(700.0), px(800.0)),
        );
        assert!(matches!(
            cfg.resolve(&OsPreferences::fallback()),
            Err(ConfigError::BoundsOrder { .. })
        ));
        // Initial size below min is rejected, not clamped.
        let small = Config::new().window(
            Window::new()
                .size(px(400.0), px(300.0))
                .min_size(px(480.0), px(360.0)),
        );
        assert!(matches!(
            small.resolve(&OsPreferences::fallback()),
            Err(ConfigError::InitialOutOfBounds { .. })
        ));
        // Initial size above max is rejected, not clamped.
        let big = Config::new().window(
            Window::new()
                .size(px(900.0), px(700.0))
                .max_size(px(800.0), px(600.0)),
        );
        assert!(matches!(
            big.resolve(&OsPreferences::fallback()),
            Err(ConfigError::InitialOutOfBounds { .. })
        ));
        // Size inside [min, max] resolves.
        let ok = Config::new().window(
            Window::new()
                .size(px(760.0), px(560.0))
                .min_size(px(480.0), px(360.0))
                .max_size(px(1024.0), px(768.0)),
        );
        let r = ok.resolve(&OsPreferences::fallback()).unwrap();
        assert_eq!(
            r.runtime.window.max_size,
            Some(aqua_platform::geometry::LogicalSize::new(1024.0, 768.0))
        );
        // Clearing limits restores the unbounded default.
        let cleared = Config::new().window(
            Window::new()
                .min_size(px(100.0), px(100.0))
                .clear_min_size()
                .max_size(px(900.0), px(900.0))
                .clear_max_size(),
        );
        let r = cleared.resolve(&OsPreferences::fallback()).unwrap();
        assert_eq!(r.runtime.window.min_size, None);
        assert_eq!(r.runtime.window.max_size, None);
    }

    #[test]
    fn nonzero_fps_only() {
        let zero = Config::new().window(Window::new()).fps(0);
        assert!(matches!(
            zero.resolve(&OsPreferences::fallback()),
            Err(ConfigError::InvalidFps { fps: 0 })
        ));
        let paced = Config::new()
            .fps(60)
            .resolve(&OsPreferences::fallback())
            .unwrap();
        assert_eq!(paced.runtime.fps_target.unwrap().get(), 60);
        let unpaced = Config::new()
            .fps(60)
            .unpaced()
            .resolve(&OsPreferences::fallback())
            .unwrap();
        assert_eq!(unpaced.runtime.fps_target, None);
    }

    #[test]
    fn glass_falls_back_under_opaque_prefs_with_honest_reason() {
        for pref in [
            OsPreferences {
                high_contrast: PrefState::On,
                ..OsPreferences::fallback()
            },
            OsPreferences {
                reduce_transparency: PrefState::On,
                ..OsPreferences::fallback()
            },
        ] {
            let r = glass().resolve(&pref).unwrap();
            assert_eq!(r.material.requested(), Material::Glass);
            assert_eq!(r.material.resolved(), aqua_platform::WindowMaterial::None);
            assert!(!r.material.is_glass());
            assert!(!r.material.reason().is_empty());
            assert_eq!(r.runtime.material, aqua_platform::WindowMaterial::None);
            // A resolved solid frame leaves the explicit transparency alone.
            assert!(!r.runtime.window.transparent);
        }
        // Unknown prefs never force the fallback by themselves.
        let plain = glass().resolve(&OsPreferences::fallback()).unwrap();
        assert_eq!(
            plain.material.is_glass(),
            material_capability(aqua_platform::WindowMaterial::Acrylic).supported
        );
    }

    #[test]
    fn solid_never_selects_glass_and_keeps_explicit_transparency() {
        let r = Config::new()
            .window(Window::new().material(Material::Solid).transparent(true))
            .resolve(&OsPreferences::fallback())
            .unwrap();
        assert!(!r.material.is_glass());
        assert_eq!(r.material.reason(), "explicit solid frame");
        assert!(r.runtime.window.transparent);
    }

    #[test]
    fn config_error_displays() {
        assert_eq!(
            ConfigError::InvalidFps { fps: 0 }.to_string(),
            "frame-rate target must be non-zero (got 0)"
        );
        assert!(ConfigError::InvalidSize {
            field: "size",
            reason: "must be positive",
        }
        .to_string()
        .contains("size"));
    }

    // -- Titlebar authoring (pure resolve; no window, no pixels) --

    #[test]
    fn titlebar_defaults_native_no_change() {
        let r = Config::new().resolve(&OsPreferences::fallback()).unwrap();
        assert_eq!(r.runtime.window.titlebar, Titlebar::Native);
        assert_eq!(r.runtime.window.titlebar_height, 40.0);
        assert!(r.runtime.window.decorations);
        assert!(r.runtime.window.minimizable);
        assert!(r.runtime.window.maximizable);
        assert!(r.runtime.window.closable);
    }

    #[test]
    fn titlebar_overlay_keeps_native_caption_buttons() {
        let r = Config::new()
            .window(Window::new().titlebar(Titlebar::Overlay))
            .resolve(&OsPreferences::fallback())
            .unwrap();
        assert_eq!(r.runtime.window.titlebar, Titlebar::Overlay);
        // Overlay keeps the native caption: still decorated.
        assert!(r.runtime.window.decorations);
    }

    #[test]
    fn titlebar_hidden_lowers_to_undecorated() {
        let r = Config::new()
            .window(Window::new().titlebar(Titlebar::Hidden))
            .resolve(&OsPreferences::fallback())
            .unwrap();
        assert_eq!(r.runtime.window.titlebar, Titlebar::Hidden);
        assert!(!r.runtime.window.decorations);
    }

    #[test]
    fn decorations_false_keeps_frameless_legacy_precedence() {
        // Legacy fully-frameless flag wins over any titlebar mode.
        for mode in [Titlebar::Native, Titlebar::Overlay, Titlebar::Hidden] {
            let r = Config::new()
                .window(Window::new().decorations(false).titlebar(mode))
                .resolve(&OsPreferences::fallback())
                .unwrap();
            assert!(
                !r.runtime.window.decorations,
                "mode {mode:?} must stay frameless"
            );
            assert_eq!(r.runtime.window.titlebar, Titlebar::Hidden);
        }
    }

    #[test]
    fn titlebar_height_validated_like_window_sizes() {
        // Invalid heights are stored without panic and rejected at resolve.
        for h in [
            px(0.0),
            px(-1.0),
            px(f32::NAN),
            px(f32::INFINITY),
            pct(50.0),
            auto(),
        ] {
            let cfg = Config::new().window(Window::new().titlebar_height(h));
            assert!(
                matches!(
                    cfg.resolve(&OsPreferences::fallback()),
                    Err(ConfigError::InvalidSize {
                        field: "titlebar_height",
                        ..
                    })
                ),
                "height {h:?} must fail at resolve"
            );
        }
        // Fractional logical px are valid heights.
        let r = Config::new()
            .window(Window::new().titlebar_height(px(44.5)))
            .resolve(&OsPreferences::fallback())
            .unwrap();
        assert_eq!(r.runtime.window.titlebar_height, 44.5);
    }

    #[test]
    fn caption_button_flags_and_repeated_titlebar_setters() {
        let r = Config::new()
            .window(
                Window::new()
                    .titlebar(Titlebar::Overlay)
                    .titlebar(Titlebar::Hidden)
                    .titlebar_height(px(10.0))
                    .titlebar_height(px(48.0))
                    .minimizable(false)
                    .maximizable(false)
                    .closable(false)
                    .closable(true),
            )
            .resolve(&OsPreferences::fallback())
            .unwrap();
        // Last titlebar call wins; Hidden still lowers to undecorated.
        assert_eq!(r.runtime.window.titlebar, Titlebar::Hidden);
        assert!(!r.runtime.window.decorations);
        assert_eq!(r.runtime.window.titlebar_height, 48.0);
        assert!(!r.runtime.window.minimizable);
        assert!(!r.runtime.window.maximizable);
        assert!(r.runtime.window.closable);
    }
}
