# Application (`App` builder)

Owner: migration agent (examples/tests/docs). Library implementation owned by
the App worker (`crates/aqua/src/app.rs`, `error.rs`); config seam owned by
the Config worker (`crates/aqua/src/config.rs`). Normative contract:
[`APP_BUILDER_CONTRACT.md`](APP_BUILDER_CONTRACT.md).

## Canonical entry point (builder-first)

```rust
use aqua::{Aqua, Context, IntoView, Material, Theme, Titlebar, px};

fn todo_app(cx: &Context) -> impl IntoView {
    let glass = cx.material().is_glass();
    let (deps, render) = todo_view(cx.scope(), Vec::new(), glass);
    cx.view(deps, render)
}

fn main() -> Result<(), aqua::Error> {
    Aqua::App()
        .config(
            Aqua::Config().window(
                Aqua::Window()
                    .title("Aqua — Tasks")
                    .size(px(760.0), px(560.0))
                    .min_size(px(480.0), px(360.0))
                    .theme(Theme::Dark)
                    .material(Material::Glass)
                    .titlebar(Titlebar::Overlay),
            ),
        )
        .run(todo_app)
}
```

UI content is authored with the same `Aqua::*()` factories
(`Aqua::Box()`, `Aqua::Text(..)`, `Aqua::Button()`, … — see
[`AUTHORING.md`](AUTHORING.md)). The bare `App` / `Config` / `Window`
builders remain available for direct use; the factories are the canonical
spelling. The low-level `ui!` macro stays compatible but is documented only
in the appendix ([`UI_MACRO.md`](UI_MACRO.md)).

Overlay titlebar (`Titlebar::Overlay`, default 40 logical px): full
client area with native caption buttons top-right; the Todo view keeps
an empty ~40px top strip (`TOP_STRIP`, same glass background, no text)
as the drag zone, so content starts below it with the `Tasks` heading
unchanged. `Native` (default) keeps the OS frame; `Hidden` is fully
frameless. The `--preview` PNG reflects client layout only — never fake
OS buttons; real native-button behavior is covered by the parent's
`scripts/windows-chrome-smoke.ps1`.

- `App::new().config(..)` is pure: nothing resolves, no OS/GPU init.
- `run(setup)` with `setup: FnOnce(&Context) -> V, V: IntoView` resolves the
  config against one OS-preferences snapshot, mounts, then enters the GUI
  loop — or, for a headless `Config`, the canonical headless one-frame
  lifecycle (mount → flush/layout/scene → dispose). Invalid config returns
  `Err(Error::Config(..))` without invoking the factory.
- `Context::scope() -> &Scope` owns setup signals; `material()` returns the
  startup `ResolvedMaterial` (`is_glass()` picks glass vs solid styling —
  no manual capability/prefs polling in `main`); `view(deps, render)`
  builds the explicit-dependency `ReactiveView`. `IntoView` is sealed over
  `Element` (static) and `ReactiveView<D, F>` only.
- Failures are typed `aqua::Error::{Config, Mount, Init, Runtime}`. CLI
  helpers that return `String` map via `Error::Init` explicitly (there is
  no `From<String>`).

## Low-level escape (still available)

Implement `aqua::Application` directly and drive `Aqua::run` /
`run_ui` / `run_ui_reactive` (see `crates/aqua/examples/showcase.rs` and
`crates/aqua/tests/facade_pump.rs`). Prefer the root `App` builder for new
code.

## Verification

- `crates/aqua/tests/app_builder.rs`: headless static + reactive `run`,
  invalid-config factory suppression, pure `resolve` fallback semantics,
  `App` vs `Application` coexistence.
- `crates/aqua/examples/todo.rs`: canonical windowed path; `--headless`
  keeps the deterministic low-level pump; `--preview <png>` keeps the
  offscreen render (baseline `artifacts/aqua-glass-layout.png`).
