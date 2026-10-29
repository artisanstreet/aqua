# Getting started with Aqua

## Prerequisites

- Rust 1.93+ (workspace minimum; host runs 1.98.1).
- Windows, or Linux with WSLg/X11 or Wayland, for windowed runs. Model and
  layout tests need no display; GPU pixel tests require a usable adapter
  (software Vulkan is sufficient).

## Run the examples

```sh
cargo run -p aqua --example showcase   # low-level Application: full runtime surface
cargo run -p aqua --example todo -- --headless   # canonical App, headless pump (CI-safe)
cargo run -p aqua --example todo                 # canonical App, real window
cargo run -p aqua --example collections -- --headless  # List/Table demo, headless pump
cargo run -p aqua --example collections            # List/Table demo, real window
```

Click a row to toggle it (click right-side × removes), type in the input
box and press Enter to add, click the blue button for the filter popover
(Escape or outside-press dismisses).

## Headless smoke (no user needed)

Time-bounded mode (preferred for mostly-static apps — a frame budget alone
never finishes without active animation):

```sh
AQUA_SMOKE_MS=1500 cargo run -p aqua --example showcase
```

The loop sleeps normally until the deadline, then quits with a production
report (`wakeups`, `requested`, `produced`, `presented_frames`, ...).
Idle validation: at fps 900 an unchanged app must show ~1 produced frame
against many sleeping wakeups — not a loop.

Frame-budget mode (active-animation benchmark, e.g. 120 frames):

```sh
AQUA_SMOKE_FRAMES=120 cargo run -p aqua --example showcase
```

For a first native-window check, `AQUA_SMOKE_FRAMES=1` proves window +
present work end to end.

On WSLg hosts with no `DISPLAY` in the environment but sockets present:

```sh
DISPLAY=:0 WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1001 \
  AQUA_SMOKE_MS=2000 cargo run -p aqua --example showcase
```

The runtime prints the smoke budget, presents that many frames, then exits
cleanly with status 0. Native Windows runs are validated the same way
(`AQUA_SMOKE_FRAMES=1` / `AQUA_SMOKE_MS=...` then check the report line).

## Tests

Targeted commands (ten agents share one host and one `target/` lock):

```sh
CARGO_BUILD_JOBS=2 cargo test -p aqua
```

This runs the facade's headless suite: config validation, scheduler
passthrough (the scheduler itself is tested in `aqua-platform`), window
event → observed-state routing, input dispatch, and scene building without
a window or GPU.

## Write an app (canonical, builder-first)

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

UI is authored with the `Aqua::*()` factories (`Aqua::Box()`,
`Aqua::Text(..)`, `Aqua::Button()`, … — see [`AUTHORING.md`](AUTHORING.md));
`Aqua::App()` / `Aqua::Config()` / `Aqua::Window()` / `Aqua::Style()` build
the entry point.

Overlay titlebar (`Titlebar::Overlay`, default height 40 logical px):
full client area with native caption buttons top-right; the Todo view
reserves an empty ~40px top strip as the drag zone, so content (kept
`Tasks` heading, no extra title text) starts below it. `Titlebar::Native`
is the default (OS frame); `Hidden` is fully frameless. Native-window
behavior is covered by the parent's `scripts/windows-chrome-smoke.ps1`;
the offscreen `--preview` reflects client layout only (never fake OS
buttons).

Keep UI state in `aqua-reactive` signals owned by your scope, commit
`DisplayItem`s into the provided `Scene`, and route popovers through
`aqua_ui::overlay::OverlayService`. See `crates/aqua/examples/todo.rs`
for the canonical path. The low-level escape is still available:
implement `aqua::Application` directly and drive `Aqua::run` /
`run_ui` / `run_ui_reactive` (see `crates/aqua/examples/showcase.rs`).
