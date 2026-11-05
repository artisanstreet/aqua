<div align="center">

# Aqua

**A native, GPU-accelerated GUI framework for Rust.**

[![License: BSD-3-Clause](https://img.shields.io/badge/license-BSD--3--Clause-blue.svg)](LICENSE)
[![Rust 1.93+](https://img.shields.io/badge/rust-1.93%2B-orange.svg)](Cargo.toml)
[![Platforms: Windows | macOS | Linux](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](docs/PLATFORM.md)
[![Status: Pre-release](https://img.shields.io/badge/status-pre--release-yellow.svg)](#status)

</div>

---

Aqua is a UI framework for building fast, beautiful desktop apps in Rust. You
write your app once, and Aqua runs it on each platform's own graphics, text,
input and accessibility stack, so it looks, feels and performs like it belongs
there.

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

## Motivation

Desktop apps usually have to choose between two options. A webview gives you a
great authoring model but ships a browser with every app: heavy on memory and
startup, and never quite native in how it scrolls, types or reads to a screen
reader. A classic cross-platform toolkit is lighter, but it tends to draw the
lowest common denominator of every platform and to re-implement what the OS
already does well.

Aqua exists to get rid of that choice.

**One codebase, native everywhere.** Everything that makes a UI a UI (state,
layout, styling, text shaping, animation and render planning) lives in a pure
core that is shared on every platform. Below it, each platform gets a thin
adapter built on its best native APIs: its own GPU API, window system, text
input and accessibility tree. Your app code never changes, and it never goes
through a generic abstraction layer on the hot path. On Windows that means
Direct3D, Win32 and UI Automation. On macOS it means Metal, AppKit and
NSAccessibility. On Linux it means Vulkan, Wayland and AT-SPI.

**Reactivity without the guesswork.** In Aqua a view declares exactly which
signals it depends on. Nothing subscribes implicitly, and nothing re-runs the
whole tree and diffs it. When a signal changes, only the views that named it
rebuild, and keyed children keep their identity, focus and edit state. The
cost of an update follows what changed, not how big your UI is, and you can
see where it happens by reading the code.

**Smooth by construction.** Animation runs as spring curves on a dedicated
render thread. While something moves, your app code does no work at all, so a
busy main thread never drops a frame, and an idle window really is idle.

**Deterministic all the way down.** The core is a state machine with no clock,
threads or I/O of its own. Every session can be recorded, replayed and checked
pixel for pixel against a CPU reference renderer, which keeps Aqua honest on
every platform it runs on.

## Features

- **Native rendering** on each platform's own GPU API, with system materials
  such as Mica, Acrylic and vibrancy, custom title bars and native window
  controls.
- **Explicit reactivity** with signals, scopes and keyed reconciliation.
- **Browser-grade styling:** composable `const` styles, design tokens, themes,
  Oklch color, flexbox and grid layout via
  [Taffy](https://github.com/DioxusLabs/taffy), shadows, blur and custom GPU
  passes.
- **First-class text:** shaping, bidi, line breaking, rich editing and IME
  composition, tested against each platform's native text renderer.
- **Virtualized collections:** lists and tables that scroll hundreds of
  thousands of rows.
- **Accessible by default:** every element maps to the platform accessibility
  tree.
- **Headless and testable:** every subsystem runs without a window or GPU.

## Installation

Aqua is not yet published on crates.io. Depend on it through git:

```toml
[dependencies]
aqua = { git = "https://github.com/artisanstreet/aqua" }
```

Aqua needs Rust 1.93 or newer.

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
the signals they declare change.

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

Aqua is split into a **pure core** that plans and **platform adapters** that
execute. Dependencies only point inward, and the boundary is machine-checked.

```
                    ┌─────────────────────────────┐
                    │            aqua             │  public API, event loop
                    └──────────────┬──────────────┘
          ┌────────────────────────┼────────────────────────┐
          ▼                        ▼                        ▼
 ┌─────────────────┐   ┌──────────────────────┐   ┌──────────────────┐
 │  Pure core      │   │  Ports               │   │  Adapters        │
 │                 │   │                      │   │                  │
 │  reactive  ui   │   │  aqua-protocol       │◀──│  GPU backends    │
 │  style  layout  │──▶│  Machine, GpuBackend │   │  window shells   │
 │  text  motion   │   │  RenderHost, Ime...  │   │  text input      │
 │  scene  graph   │   │                      │   │  accessibility   │
 └─────────────────┘   └──────────────────────┘   └──────────────────┘
   shared on every                                  one set per
   platform                                         platform
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
| `aqua-gpu-ref` | CPU reference renderer: oracle and headless backend |
| `aqua-verify` | Architecture enforcement, heap gates, differential tests |

Each platform plugs in through the same ports:

| | Windows | macOS | Linux |
|---|---|---|---|
| Rendering | Direct3D 11 / 12 | Metal | Vulkan |
| Windowing | Win32 | AppKit | Wayland, X11 |
| Text input | Text Services Framework | NSTextInputClient | IBus, Fcitx |
| Accessibility | UI Automation | NSAccessibility | AT-SPI |

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

## Status

Aqua is pre-release and the API will change. We do not publish speed
comparisons until they come from matched, reproducible workloads; see
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) for what is measured today.

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
