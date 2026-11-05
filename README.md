<div align="center">

# Aqua

**A native, GPU-accelerated GUI framework for Rust.**

Explicit reactivity · Render-thread animation · Browser-grade styling · Deterministic by design

[![License: BSD-3-Clause](https://img.shields.io/badge/license-BSD--3--Clause-blue.svg)](LICENSE)
[![Rust 1.93+](https://img.shields.io/badge/rust-1.93%2B-orange.svg)](Cargo.toml)
[![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6.svg)](docs/PLATFORM.md)
[![Status: Pre-release](https://img.shields.io/badge/status-pre--release-yellow.svg)](#status)

[Getting started](docs/GETTING_STARTED.md) ·
[Authoring guide](docs/AUTHORING.md) ·
[Architecture](ARCHITECTURE.md) ·
[Examples](crates/aqua/examples)

</div>

---

Aqua is a desktop UI framework that goes straight to the platform: Direct3D for
rendering, Win32 for windowing, DirectWrite-grade text, Text Services Framework
for input methods and UI Automation for accessibility. No webview, no
cross-platform abstraction layer in the hot path.

Under the native adapters sits a **pure, sans-IO core**. The whole runtime is a
state machine, `step(Input) -> Output`, that reads no clock, spawns no threads
and touches no files. That makes every frame reproducible: record a session,
replay it, seek through it, and diff it pixel for pixel against a CPU reference
renderer.

```rust
use aqua::{Aqua, Context, IntoView, Material, Theme, px};

fn counter(cx: &Context) -> impl IntoView {
    let (count, set_count) = cx.scope().signal(0);

    cx.view((count,), move |(count,)| {
        let increment = set_count.clone();

        Aqua::Box()
            .child(Aqua::Text(format!("Clicked {count} times")).build())
            .child(
                Aqua::Button()
                    .on_activate(move || increment.update(|n| *n += 1))
                    .child(Aqua::Text("Increment").build())
                    .build(),
            )
            .build()
    })
}

fn main() -> Result<(), aqua::Error> {
    Aqua::App()
        .config(
            Aqua::Config().window(
                Aqua::Window()
                    .title("Counter")
                    .size(px(480.0), px(320.0))
                    .theme(Theme::Dark)
                    .material(Material::Glass),
            ),
        )
        .run(counter)
}
```

## Why Aqua

- **Native from the first pixel.** Direct3D 11 by default, Direct3D 12 opt-in,
  real Win32 windows with Mica and Acrylic materials, overlay title bars and
  native caption buttons.
- **Reactivity you can read.** A view lists its dependencies explicitly. There
  are no hidden subscriptions and no whole-tree diffing: invalidation cost
  follows what changed, not how big the view is.
- **Animation off the main thread.** Motion runs as analytic spring tracks on a
  dedicated render thread. While something animates, your app code does no
  work at all.
- **Styling on the level of a browser engine.** Composable `const` styles,
  design tokens, themes, Oklch color, flexbox and grid layout via
  [Taffy](https://github.com/DioxusLabs/taffy), shadows, blur and custom GPU
  passes without forking the framework.
- **Text that holds up.** Shaping, bidi, line breaking, editing, IME
  composition and glyph rasterization, gated by pixel diffs against
  DirectWrite.
- **Deterministic and testable.** Every subsystem runs headless. A CPU
  reference renderer is the oracle for the GPU path, and recorded input
  sessions replay to identical output.
- **Idle means idle.** Scheduling is demand-driven. An unchanged window sleeps
  instead of looping.

## Installation

Aqua is not yet published on crates.io. Depend on it through git:

```toml
[dependencies]
aqua = { git = "https://github.com/artisanstreet/aqua" }
```

**Requirements:** Rust 1.93 or newer. Windowed apps target Windows 10 and 11.
The pure crates, the reference renderer and headless runs work on Linux too.

## A tour of the API

### Elements

UI is built with the `Aqua::*()` factories. Every element takes children and a
list of styles.

```rust
Aqua::Box()
    .styles([CARD])
    .child(Aqua::Text("Settings").styles([HEADING]).build())
    .child(Aqua::Checkbox("Enable sync").checked(true).build())
    .child(Aqua::Slider(0.5).build())
    .build()
```

Available out of the box: `Box`, `Text`, `Button`, `Checkbox`, `Slider`,
`Dropdown`, `Tabs`, `Scroll`, `List`, `Table`, `Media`, `Canvas`,
`Viewport3D`, `Fragment` and `Dynamic`.

### Styles

Styles are plain `const` values. Pass several and later entries win per field,
so variants compose without inheritance tricks.

```rust
use aqua::{Aqua, Style, pct, px};
use aqua::prelude::Oklch;

const ACCENT: Oklch = Oklch::new(0.65, 0.17, 252.0);

const BUTTON: Style = Aqua::Style()
    .padding(px(12.0))
    .radius(px(10.0))
    .background(Oklch::WHITE.with_alpha(0.06));

const PRIMARY: Style = Aqua::Style().background(ACCENT);

Aqua::Button().styles([BUTTON, PRIMARY])
```

### State

Signals live in a scope and come as a read/write pair. Views rebuild only when
the signals they declare change, and keyed children keep their identity, focus
and edit state across rebuilds.

```rust
let (todos, set_todos) = cx.scope().signal(Vec::<Todo>::new());
let (draft, set_draft) = cx.scope().signal(String::new());

Aqua::Text("")
    .bind((draft.clone(), set_draft.clone()))
    .placeholder("New task")
    .on_submit(move || set_todos.update(|v| v.push(Todo::new(draft.get()))))
    .build()
```

### Collections

`List` and `Table` virtualize out of the box, with variable row heights,
scroll anchoring, focus pinning and `fr` column tracks. The `collections`
example scrolls 100,000 rows.

### Windows

Windows are configured declaratively: title, size, minimum size, theme,
material (`Glass`, solid fallback) and title bar style (`Native`, `Overlay`,
`Hidden`). Multiple windows share one render thread.

## Examples

```sh
cargo run -p aqua --example todo            # reactive todo list on a glass window
cargo run -p aqua --example collections     # 100k-row virtual list and sortable table
cargo run -p aqua --example showcase        # the full runtime surface: media, 3D, IME, a11y
cargo run -p aqua --example multi_window    # several windows, one render thread
cargo run -p aqua --example editor_shell    # an editor-style application shell
```

`todo` and `collections` accept `-- --headless` to run their scripted input
without a display, which is how they run in CI.

## Architecture

Aqua is split into **pure crates** that plan and **adapters** that execute.
Dependencies only point inward, and the boundary is machine-checked.

```
                    ┌─────────────────────────────┐
                    │            aqua             │  public API, event loop
                    └──────────────┬──────────────┘
          ┌────────────────────────┼────────────────────────┐
          ▼                        ▼                        ▼
 ┌─────────────────┐   ┌──────────────────────┐   ┌──────────────────┐
 │  Pure core      │   │  Ports               │   │  Adapters        │
 │                 │   │                      │   │                  │
 │  reactive  ui   │   │  aqua-protocol       │◀──│  gpu-d3d11/12    │
 │  style  layout  │──▶│  Machine, GpuBackend │   │  gpu-ref (CPU)   │
 │  text  motion   │   │  RenderHost, Ime...  │   │  shell-win32     │
 │  scene  graph   │   │                      │   │  ime-tsf a11y-uia│
 └─────────────────┘   └──────────────────────┘   └──────────────────┘
   no I/O, no clock,                                all unsafe and
   no threads, no unsafe                            OS code lives here
```

| Crate | Role |
|---|---|
| `aqua` | Public API, composition root and the event loop |
| `aqua-reactive` | Declared-dependency graph, scopes, batching |
| `aqua-ui` | Element builders, node tree, keyed reconciliation, overlays |
| `aqua-style` | Styles, tokens, cascade, themes |
| `aqua-layout` | Flexbox and grid on Taffy |
| `aqua-text` | Shaping, bidi, line breaking, editing, rasterization |
| `aqua-motion` | Motion targets and analytic spring tracks |
| `aqua-scene` / `aqua-graph` | Display list, damage, render graph planner |
| `aqua-runtime` | The sans-IO frame pipeline and render side |
| `aqua-gpu-d3d11` | Direct3D 11 backend (Windows default) |
| `aqua-gpu-d3d12` | Direct3D 12 backend (opt-in) |
| `aqua-gpu-ref` | CPU reference renderer: oracle and headless backend |
| `aqua-shell-win32` | Windows, input, DPI, chrome, clipboard, dialogs |
| `aqua-ime-tsf` / `aqua-a11y-uia` | Input methods and accessibility |
| `aqua-verify` | Architecture enforcement, heap gates, differential tests |

The full design, with its decision records, is in
[`ARCHITECTURE.md`](ARCHITECTURE.md).

## Documentation

| Guide | |
|---|---|
| [Getting started](docs/GETTING_STARTED.md) | Install, run the examples, write a first app |
| [Application](docs/APPLICATION.md) | The `App` builder, config and lifecycle |
| [Authoring](docs/AUTHORING.md) | Elements, events and composition |
| [Styling](docs/STYLING.md) | Styles, tokens and themes |
| [Reactivity](docs/REACTIVITY.md) | Signals, scopes and views |
| [Collections](docs/COLLECTIONS.md) | Virtualized lists and tables |
| [Text](docs/TEXT.md) | Shaping, editing and IME |
| [Overlays and motion](docs/OVERLAYS_AND_MOTION.md) | Popovers, springs and presence |
| [Window chrome](docs/WINDOW_CHROME.md) | Title bars, materials and caption buttons |
| [GPU extensions](docs/GPU_EXTENSIONS.md) | Custom render passes |
| [Performance](docs/PERFORMANCE.md) | Counters, budgets and how we measure |

## Building from source

```sh
git clone https://github.com/artisanstreet/aqua
cd aqua
cargo test --workspace
```

Cross-compiling a Windows build from WSL is covered in
[`docs/WINDOWS_BUILD.md`](docs/WINDOWS_BUILD.md).

## Status

Aqua is pre-release and the API will change. Windows is the first platform;
other platforms will follow through new adapters behind the same ports. We do
not publish speed comparisons until they come from matched, reproducible
workloads; see [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) for what is
measured today.

## Contributing

Issues and pull requests are welcome. Before opening a PR, run
`cargo test --workspace`. The architecture rules are enforced by
`aqua-verify`, so a change that breaks a boundary fails in CI and not in
review.

## License

Aqua is licensed under the [BSD 3-Clause License](LICENSE).

<div align="center">
<sub>Built by <a href="https://github.com/artisanstreet">Artisan Street</a>.</sub>
</div>
