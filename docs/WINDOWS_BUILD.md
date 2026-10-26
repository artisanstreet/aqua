# Building a Windows Aqua app from WSL

Aqua can reuse the same Nix cross-compilation shell as Artisan Street's
editor. This builds Aqua only; it does not build, install, or restart Artisan.

```sh
bash scripts/build-windows.sh
```

The default builds the reactive Todo app in release mode and writes
`dist/windows/aqua-todo.exe` plus its SHA-256 checksum. Launch that executable
from Windows. `--headless` runs its deterministic input/state checks instead
of opening a window.

Other builds:

```sh
bash scripts/build-windows.sh todo dev
bash scripts/build-windows.sh showcase release
bash scripts/build-windows.sh collections release
```

Set `AQUA_EDITOR_CHECKOUT` if the editor lives somewhere other than
`~/projects/artisanstreet/editor`. Nix and access to the checkout's pinned
flake inputs are required.

## How the editor builds

The inspected checkout is `/home/sander/projects/artisanstreet/editor`:

- `flake.nix` imports nixpkgs with the `mingwW64` cross system.
- `nix/workspace.nix` exposes `cargo-windows`, targeting
  `x86_64-pc-windows-gnu`, with MinGW GCC, binutils, pthreads, and mcfgthreads.
- That shell sets the Cargo linker, target C/C++ tools, and Rust flags. The
  runtime is linked statically, including `libmcfgthread.a`, with Windows
  system imports `ntdll` and `kernel32`.
- `nix/dev.sh` detects WSL and builds the Windows editor through
  `nix develop …#cargo-windows --command cargo build`, with a separate target
  directory. Its installation/staging steps are separate from compilation.

The Aqua script follows that compilation path and uses its own target and
output directories. It inherits the editor's **x86-64-v3 CPU baseline**;
these binaries require a compatible CPU, including AVX2. It does not use
MSVC, Wine, or a native Windows Rust installation.

## Verified artifact

The release Todo build was compiled using this shell's Rust 1.98.0 and
tested as a native Windows process. Both checks exited with code 0:

- `aqua-todo.exe --headless`: type, Add, toggle, and Remove assertions pass.
- Windowed run with `AQUA_SMOKE_MS=1500`: one GPU frame presented,
  `render_errors=0`, then clean shutdown. This verifies execution and GPU
  submission; it is not a visual or screen-reader test.

The PE uses the Windows GUI subsystem, so double-clicking it does not open
an extra console. Its imports are Windows system DLLs only. A copy is at
`C:\Users\sander\Downloads\Aqua\aqua-todo.exe`, with its checksum and
`native-smoke.log` alongside it. Build outputs in `dist/` are ignored by Git.

## Daily UI refresh (2026-10-26)

The styled app is also exported as
`C:\Users\sander\Downloads\Aqua\aqua-daily.exe`, with a checksum and
`daily-smoke.log`. Both its headless task lifecycle and native GPU window
smoke passed with exit code 0. The older Todo executable can remain open.

The refresh adds a padded field, focus outline, muted placeholder,
Enter-to-add, styled task rows, and an empty state. Editing fixes include
single key dispatch, horizontal caret tracking, correct padding and DPI
geometry, IME-safe submission, and actual GPU clipping. A renderer fix
keeps borders from replacing their interior fill.

Reproduce the visual preview with sample tasks:

```sh
cargo run -p aqua --example todo -- --preview artifacts/aqua-daily.png
```

Regression checks cover field geometry and caret scrolling at 1×, 1.5×,
and 2× scale, single key/submit dispatch, IME composition, and border pixels.
The strict validation script passes all 11 checks with no GPU skips.

## Native Backspace regression (2026-10-26)

Windows supplies control-key text alongside key identity (Backspace U+0008,
Enter CR, Tab TAB). The adapter previously forwarded that text after the
editing command, reinserting a control character that rendered as a box.
The canonical key translator now rejects control-key text and suppresses
shortcut commits, while retaining logical shortcut identity and AltGr text.
Multiline Enter inserts a newline through the editing command instead.

`crates/aqua/tests/windows_key_input.rs` replays these payloads through the
platform translator and real UI host. `examples/windows_input_smoke.rs`
and `scripts/windows-input-smoke.ps1` additionally verify the native Windows
message path in an isolated child: type `ABc`, Backspace once → `AB`, then
delete to empty; Backspace/Delete/Tab/Enter never reinsert control text.
The native replay exited 0 with `WINDOWS_INPUT_OK`.

The corrected release is copied to
`C:\Users\sander\Downloads\Aqua\aqua-daily-fixed.exe` so an older running
executable does not prevent delivery. Restart using that new file.

## Glass Tasks demo (2026-10-26)

The current release is `C:\Users\sander\Downloads\Aqua\aqua-glass.exe`.
It removes the eyebrow labels, descriptive subtext, input hint, and footer,
leaving Tasks, a compact count, and the task controls. Shared `Style` values
use alpha for the page tint, fields, buttons, and rows.

`todo` requests `WindowMaterial::Acrylic` and dark native chrome through
`config.window.theme`. Unsupported material or opaque OS preferences select
the solid page background. The offscreen `--preview` path deliberately
renders that opaque fallback: desktop Acrylic belongs to the Windows
compositor and is not fabricated in a headless screenshot.

Transparent Windows windows now use wgpu's DX12 `DxgiFromVisual` swapchain
and `WS_EX_NOREDIRECTIONBITMAP`. The original Vulkan HWND surface reported
`alpha=Opaque`, which cannot show desktop content. The native replacement
reports `Dx12, alpha=PreMultiplied, format=Bgra8UnormSrgb`, with Acrylic
applied, exit 0, one presented frame, and zero render errors in the finite
smoke run. These are execution/composition-path checks, not screenshot or
screen-reader evidence. See `artifacts/glass-native-smoke.log`.

Presentation also preserves encoded-premultiplied color on both Unorm and
sRGB attachments. A GPU regression verifies half-alpha color bytes and a
fully clear region, preventing bright fringes on translucent surfaces.


## Factory and collections builds (2026-10-26)

`Aqua::Box()` / `Aqua::Text(..)` and iterator `.children()` are now the canonical
Todo authoring path. Build the full dataset demo with:

```sh
bash scripts/build-windows.sh collections release
```

The output is `dist/windows/aqua-collections.exe`. The demo supplies 100,000
list items and 100,000 table rows with opt-in virtualization. Both it and the
migrated Todo passed native Windows smoke runs (exit 0, one GPU presentation,
zero render errors). Repeat run evidence is recorded in
`artifacts/collections-native-smoke.log`; the offscreen preview is
`artifacts/aqua-collections-final.png`. These logs verify execution and frame
submission, not native visual appearance or screen-reader behavior.

Copies are available at:
- `C:\Users\sander\Downloads\Aqua\aqua-factory-tasks.exe`
- `C:\Users\sander\Downloads\Aqua\aqua-collections.exe`
