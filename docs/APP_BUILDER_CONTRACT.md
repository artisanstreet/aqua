# App builder integration contract

Canonical user entry point:

```rust
App::new().config(Config::new().window(
    Window::new().title("Aqua — Tasks")
        .size(px(760.0), px(560.0))
        .min_size(px(480.0), px(360.0))
        .theme(Theme::Dark).material(Material::Glass)
)).run(todo_app)
```

Private-field authoring builders `Config` and `Window` live in `aqua::config`.
`Theme::{System,Light,Dark}`, `Material::{Solid,Glass}` are semantic authoring
values. Material belongs to Window. Lower-level `AquaConfig` remains for
existing runtime internals; do not duplicate the runtime.

One canonical App::run accepts setup FnOnce(&Context) -> V, where V is a
public sealed IntoView implemented for Element (static) and
ReactiveView<D,F> (explicit dependencies). Context exposes scope() -> &Scope,
material() -> &ResolvedMaterial, and view(deps, render) -> ReactiveView<D,F>.
Keep reactivity explicit, no hidden subscriptions. `.run(todo_app)` works
with `fn todo_app(cx: &Context) -> impl IntoView` returning cx.view(...).
Low-level callback trait becomes Application; root App is the new builder.
Existing Aqua::run / run_ui / run_ui_reactive remain lower-level compatibility.

ResolvedMaterial reflects startup policy/capability resolution; it does NOT
claim DWM has already applied before the window exists. It exposes is_glass()
for UI fallback styling and requested/resolved/reason accessors as useful.
The adapter maps Glass -> Windows Acrylic when supported and transparency /
high-contrast preferences permit; elsewhere resolve Solid with honest reason.
Do not invent macOS/Linux blur backends.

Config owner must publish exact conversion/error signatures in this file ASAP.
Frozen seam (config owner, published 2026-10-26; lives in `aqua::config`):

```rust
impl Config {
    pub fn resolve(&self, prefs: &OsPreferences) -> Result<ResolvedConfig, ConfigError>;
}
pub struct ResolvedConfig {
    pub runtime: AquaConfig,       // lowered runtime config for the App worker
    pub material: ResolvedMaterial, // startup policy/capability resolution
}
impl ResolvedMaterial {
    pub fn requested(&self) -> Material;        // authoring Solid/Glass
    pub fn resolved(&self) -> WindowMaterial;  // platform candidate (None = solid)
    pub fn reason(&self) -> &'static str;      // honest policy/capability line
    pub fn is_glass(&self) -> bool;            // resolved != None; NOT "DWM applied"
}
pub enum ConfigError { InvalidSize { .. }, BoundsOrder { .. }, InitialOutOfBounds { .. }, InvalidFps { .. } }
impl std::fmt::Display for ConfigError {}
impl std::error::Error for ConfigError {}
```

Titlebar seam (config owner; platform owns `window::Titlebar::{Native, Overlay, Hidden}`,
re-exported via `aqua::config`; root export owned by runtime worker):

```rust
impl Window {
    pub fn titlebar(mut self, titlebar: Titlebar) -> Self;      // Native default
    pub fn titlebar_height(mut self, height: Length) -> Self;   // px(), finite, > 0
    pub fn minimizable/maximizable/closable(mut self, v: bool) -> Self;
}
// Lowered: WindowOptions { titlebar: Titlebar, titlebar_height: f64, minimizable, maximizable, closable }
```

Layering (resolved in `Config::resolve`, applied by the runtime): legacy
`decorations(false)` stays fully frameless and wins over any titlebar mode
(coerced to `Hidden`); explicit `Hidden` lowers to undecorated; `Overlay`
keeps native caption buttons with full client content (Windows-only behavior,
runtime falls back to `Native` elsewhere); `Native` passes through unchanged.
Validation: `titlebar_height` takes `px()` only — zero/negative/NaN/infinite/
`Pct`/`Auto` are stored without panic and rejected at resolve with
`ConfigError::InvalidSize { field: "titlebar_height", .. }`.
```

ResolvedConfig has public runtime: AquaConfig and material: ResolvedMaterial.
ConfigError belongs to config.rs; App worker's Error wraps it. Validate
finite positive pixel window sizes, bounds ordering, nonzero FPS. Store invalid
builder values until resolve rather than panic inside setters. No eager OS/GPU
initialization in App/Config/Window builders.

Ownership:
- Config worker: config.rs and config-only tests/docs; no lib.rs/runtime edits.
- App worker: new app.rs/error.rs as needed, runtime.rs, ui_app.rs, lib.rs;
  owns trait rename there and public exports. No config/example/test edits.
- Migration worker: examples/tests and README/docs (except this contract),
  no library implementation edits. Rename old App trait imports to Application.
- Parent: integration/review/final validation and native build/smoke.

Keep current DirectComposition DX12 + premultiplied surface + Windows Acrylic
path intact. Same Todo pixels and input behavior should survive migration.
No pushes, no resets, all source files are currently untracked baseline.
