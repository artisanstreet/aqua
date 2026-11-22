# Aqua platform adapter (`aqua-platform`)

Status note (2026-11-21): `aqua-platform` was replaced by the native shells
(`ARCHITECTURE.md` record 14.4). Windows uses `aqua-shell-win32`; Linux uses
`aqua-shell-wayland` and `aqua-portal` (record 14.LW). The tables below describe
the historical crate; the Linux section at the end describes the current Linux
behaviour.

Owner: platform agent. Windows-first; one portable API over **winit 0.30
(stable, crate version `0.30.12+`)** plus native Windows (DWM) and OS-service
integrations. Facade (`aqua`) owns the event loop and `ApplicationHandler`;
this crate provides the helpers it must use.

Canonical vocabulary lives in `aqua-core` (`geometry`, `event`, `semantic`):
platform translates native occurrences into it. New input code uses
`events::translate_*_core`; the provisional `Aqua*` enums stay for the
existing facade call sites until the facade migrates (see relay note at the end).

## Public API (facade integration)

| Item | Path | Notes |
|---|---|---|
| `WindowOptions`, `WindowRequest`, `WindowEvent`, `ObservedWindowState`, `ThemePreference` | `window` | Requests (intent) vs observed state (compositor fact). `validated()` rejects empty sizes. `WindowEvent::ThemeChanged` added (non-exhaustive matching required). `ObservedWindowState::new` preferred over literals. |
| `AquaWindowId`; core `Point/Size/Rect` re-exported; `LogicalSize/Position`, `PhysicalSize` | `geometry` | Window identity is adapter-owned. `f64` DIP sizes convert explicitly (`as_core_size`, `as_core_point`, `From`). |
| `window_attributes(&opts)` | `host` | Build `winit::window::WindowAttributes`; pass to `ActiveEventLoop::create_window`. Opens no window — headless-safe. |
| `translate_window_event(&ev, id, scale)` | `host` | `winit::event::WindowEvent` → `Option<WindowEvent>`. Total (wildcard arm); now also maps `ThemeChanged`. Input/IME/scale detail → `None`, routed via `events::translate_*`. |
| `surface_signal(&ev)` | `host` | `SurfaceSignal{resize_gpu, scale_stale, material_recheck, skip_production}` — one place for DPI/material/focus follow-ups. |
| `translate_modifiers/mouse_button_core/key_event_core/ime_core`, `to_core_point`, `pointer_contact`, `translate_theme`, `FileDrop` | `events` | Canonical translators (core types out). `key_event_core` returns `(KeyEvent, Option<committed text>)`; text routes once as `TextInput`, suppressed during IME. |
| `to_winit_cursor`, `ImeAreaWindow` impl for `winit::window::Window` | `host` / `services` | Cursor mapping; `set_ime_cursor_area` validates before touching the OS. |
| `control_flow_for_next_wakeup(next)` | `host` | `WaitUntil(deadline)` / `Wait` from `Scheduler::next_wakeup`. |
| `Scheduler`, `FrameReason`, `FrameStats` | `scheduler` | Demand-driven deadlines; app guide: `docs/SCHEDULER.md`. |
| `WindowMaterial`, `material_capability`, `material_needs_transparent_window`, `DWM_BACKDROP_MIN_BUILD` | `material` | Semantic intent → honest capability. |
| `adapters::apply_material`, `windows::apply_titlebar_theme`, `windows::windows_build_number` | `adapters` | Windows: real DWM apply + dark-frame theming + build probe. Elsewhere: explicit fallback. |
| `Clipboard`, `SystemClipboard`, `MemoryClipboard`, `app_clipboard`, `ClipboardBackend` | `services` | Native OS clipboard default; memory is fallback/test only. Fallible (`ClipboardError`). |
| `DialogRequest`, `FileFilter`, `dialogs_capability` | `dialogs` | Native file dialogs, parented asynchronous results and explicit multi-file selection (rfd, portal-based on Linux — no GTK). |
| `DisplayInfo` | `monitor` | Current monitor bounds, work area, scale and constrained centering. |
| `OpenTarget` | `shell` | Validated URL, path and reveal actions with native error reporting. |
| `NotificationCenter`, `AppIdentity`, `Notification` | `notifications` | Windows toasts, actions, replacement and withdrawal; explicit unsupported result elsewhere. |
| `TaskbarBadge` | `badge` | Validated native Windows overlay rendering; the facade replays retained state after Explorer restarts. |
| `OsPreferences`, `query_preferences`, `should_reduce_motion`, `prefers_opaque_frame` | `prefs` | Reduced motion/transparency, high contrast (Windows SPI+registry; env overrides; explicit Unknown elsewhere). |
| `A11yNode`, `build_tree_update`, `map_role`, `pack/unpack_node_id`, `a11y_capability` | `a11y` | Portable AccessKit vocabulary (common dep). Action payloads preserved (`A11yActionData`: value/numeric/selection/point/offset/custom). |
| `A11yBridge`, `Bridge*Handler`, `WakeFn` | `a11y` (common) | Wake contract: every bridge event (action, initial-tree request, deactivation) wakes EXACTLY once AFTER lock release — unit-tested (`bridge_wakes_once_per_event_without_lock_held`). |
| `A11yAdapter::attach_with_waker` | `a11y_winit` (`cfg(windows)`) | Live UIA bridge: invisible-first attach, REQUIRED event-loop waker (proxy), event forwarding, snapshot push, action drain. AT clicks arrive as requests, never via mouse input. |
| `Titlebar`, `WindowOptions.titlebar/titlebar_height/minimizable/maximizable/closable` | `window` | Canonical chrome config (defaults: Native, 40px, all on). |
| `WindowsChrome::install`, `DragRegions`, `set_drag_regions/set_drag_rects` | `chrome` (+ `chrome_winit` live guard on Windows, stub elsewhere) | Native overlay chrome; user doc `docs/WINDOW_CHROME.md`. |

## Scheduler contract

- `set_fps_target(Option<NonZeroU32>)`: `None` = unpaced/demand-only.
  Any non-zero value accepted; interval clamped ≥ 1µs. Target changes clear
  pacing gates so the new policy applies to pending work immediately.
- `request_frame(id, reason)` merges `FrameReason`, coalesces repeats.
  `request_animation_deadline(id, t)` wakes animation-only windows at
  `max(gate, t)` (single fused wakeup); damage + future deadline draws now.
- `ready_windows(now)` → `mark_produced(ids, now)` → `mark_presented(id)`
  on ACTUAL presentation completion (next acquire fences the previous
  present — see `docs/SCHEDULER.md`; submission-time release bounds only a
  nominal counter). Cap (`set_max_in_flight`, default 2) defers excess.
- `next_wakeup(now)` → `None` when idle AND when the only pending work is
  backpressure-blocked (completion wakes it, never a timer — no busy-loop);
  other windows are unaffected by one blocked window.
- Deterministic: no clock reads inside. Recovery covered: overload→idle,
  resize storms, remove-during-flight, present-without-produce saturation,
  deadline supersede (earliest wins).

## Native materials (verified against MS docs)

- `Mica` → `DWMSBT_MAINWINDOW (2)`, `MicaAlt` → `DWMSBT_TABBEDWINDOW (4)`
  (stronger-tinted alternate for tabbed title bars — NOT an inactive
  variant), `Acrylic` → `DWMSBT_TRANSIENTWINDOW (3)`, via
  `DwmSetWindowAttribute`, min client Windows 11 build 22621 (probed via
  `windows_build_number()`, registry read; unknown ≠ supported).
- Complete setup beyond the attribute (per Mica guidance): transparent
  window (`WindowOptions.transparent`) + alpha GPU surface cleared
  transparent + transparent content layers. NO `DwmExtendFrameIntoClientArea`
  on this path (DWM draws behind entire window bounds); legacy
  `SetWindowCompositionAttribute` NOT used (undocumented).
- Dark frames: `apply_titlebar_theme(hwnd, dark)` (`DWMWA_USE_IMMERSIVE_DARK_MODE`).
- High contrast / transparency-off / deactivation → solid fallback
  (`prefers_opaque_frame`), matching OS behavior.
- Null handles rejected; failures returned. Compile-only checks cannot prove
  the visual — verify on a real Win11 22621+ host. Native runtime NOT tested.

## Dependency footprint (actual `cargo tree`, bloat audit)

| Dep | Scope | Linux cost | Notes |
|---|---|---|---|
| `winit 0.30`, `bitflags`, `aqua-core` | common | baseline | windowing foundation |
| `arboard 3` | common | x11rb/wayland (already in tree via winit) | native clipboard; fails clean headless |
| `rfd 0.17` | common | portal-based (~25 small crates), **no GTK** (verified in tree) | native dialogs; `dialogs_available()` gates headless |
| `accesskit 0.25` | common | schema only (small) | tree building + tests compile headless |
| `accesskit_winit 0.34` | **`cfg(windows)` only** | 0 on Linux (was ~150 AT-SPI/zbus/async pkgs) | UIA adapter; Linux tree 204 → 108 pkgs |
| `windows 0.62`, `raw-window-handle` | `cfg(windows)` only | 0 on Linux | DWM/SPI/registry/HWND |

## Platform scope

The Windows Editor port has the platform contracts listed in
[EDITOR_READINESS.md](EDITOR_READINESS.md). Typed in-app drag-and-drop and
popup menus are implemented by `aqua-ui`; native file drops include their
actual client coordinates. The facade drains bounded worker mailboxes and
applies window commands, per-window frame targets and live vsync changes.

A software-rendering fallback policy remains outside the Windows
qualification; macOS materials fall back to solid (record 14.MA, MD9). Native UIA tests establish API behavior;
screen-reader speech quality and physical CJK candidate-window placement
remain release qualification tasks.

## Linux (Wayland and Vulkan)

Linux is a supported target on Wayland only; there is no X11 path. The details
and evidence are in `ARCHITECTURE.md` record 14.LW. In short:

| Service | Linux implementation | When it is missing |
|---|---|---|
| Windows, input, chrome, cursors | `aqua-shell-wayland` over `xdg-shell`; server-side decorations through xdg-decoration | without xdg-decoration, `Titlebar::Native` becomes a 40 px drag band (logged once); `native::run` returns an error without a compositor |
| Rendering | `aqua-gpu-vulkan`, Vulkan 1.3; custom passes through `libdxcompiler.so` | custom passes report `Unsupported("custom passes")` |
| Clipboard, primary selection, file drops | `wl_data_device` and `zwp_primary_selection_v1`; selecting text sets the primary selection and a middle click pastes it | `ClipboardBackend::MemoryFallback`; primary selection does nothing |
| Text input | `aqua-ime-wayland` over text-input-v3 | typing still works, without composition |
| Fonts | fontconfig, loaded at run time, configured generic families first | a walk of the XDG font directories, reported with one `aqua:` line |
| Preferences, open, reveal, notifications, badge | `aqua-portal` over the session bus (portal Settings, OpenURI with the window's exported handle and an activation token, FileManager1, Notification, launcher badge) | fallback preferences; `OpenError::Unsupported`, `NotificationError::Unsupported`, `BadgeError::Unsupported` |
| File dialogs | rfd through the FileChooser portal, parented to the window | `DialogError::Unavailable` without a display; a portal that cannot answer ends the dialog with no paths |
| Accessibility | `aqua-a11y-atspi` (AT-SPI through `accesskit_unix`) | no screen-reader tree |
| Memory pressure | PSI trigger on `/proc/pressure/memory`, else `MemAvailable` below 5 % | no low-memory trims |
| Caches | `$XDG_CACHE_HOME/aqua` (else `~/.cache/aqua`): font fallback index and `pipelines-vulkan.cache` | caches are not persisted |

Unsupported on Wayland, returned as `Unsupported` with a reason: global window
position, restoring a minimized window, always-on-top, sending a window to the
back, raw mouse input, and on some compositors minimizing, activation and
exported handles. Window materials other than solid report a fallback. There is
no `local_time` (the Windows locale formatter) and `file_icon` returns `None`.

Tests never reach the user's desktop: `.cargo/config.toml` points the session
bus, accessibility bus, runtime directory and Wayland display at dead ends and
the cache directory at `target/aqua-test-cache`, and
`aqua-verify/tests/isolation.rs` enforces that. Wayland tests run against a
private headless sway and a private D-Bus daemon (`docs/GETTING_STARTED.md`).

## macOS (AppKit and Metal)

macOS runs on Apple Silicon only (`aarch64-apple-darwin`), macOS 14.0 or newer.
The details and evidence are in `ARCHITECTURE.md` record 14.MA; everything
Mac-only is proven on GitHub `macos-15` runners. In short:

| Service | macOS implementation | When it is missing |
|---|---|---|
| Windows, input, chrome, cursors | `aqua-shell-appkit`: NSWindow and NSView with an explicit-time loop, backing scale and occlusion, keyboard through `UCKeyTranslate`, precise and line scrolling, `NSCursor`, `Titlebar::Overlay` with the native traffic lights | `native::run` returns `ShellError::AppKit` without a window server session |
| Rendering | `aqua-gpu-metal`, MSL built from the shared HLSL through SPIRV-Cross; custom passes through `libdxcompiler.dylib` and `libspirv-cross-c-shared.dylib` in the bundle's `Contents/Frameworks`, beside the executable, or in `AQUA_DXCOMPILER` and `AQUA_SPIRV_CROSS_C` | custom passes report `Unsupported("custom passes")` with one `aqua:` line naming every path tried |
| Clipboard, file drops | `NSPasteboard` (text, PNG and TIFF images, file URLs); file URL drags with view-local positions | none; tests use a private named pasteboard |
| Text input | `aqua-ime-appkit` over `NSTextInputClient`: marked text, commit, cancel, reconversion by the input method | none |
| Fonts | Core Text: available face files and the system UI font's cascade per generic family, read through skrifa | a walk of `/System/Library/Fonts`, `/Library/Fonts` and `~/Library/Fonts`, reported with one `aqua:` line |
| Preferences | `NSWorkspace` reduce motion, reduce transparency and increase contrast, the effective appearance, followed live | the defaults |
| Open, reveal, file icons | `NSWorkspace` `openURL:`, `activateFileViewerSelectingURLs:` and `iconForFile:` | `OpenError::Desktop` when the workspace refuses; `file_icon` returns `None` for a missing path |
| File dialogs | `NSOpenPanel` and `NSSavePanel` as sheets on the parent window; cancelling returns no paths | `ShellError::Dialog` when the panel cannot be shown or ends without a result |
| Notifications | `UNUserNotificationCenter`, only inside an app bundle with a `CFBundleIdentifier` (`scripts/macos-bundle.sh`) | `NotificationError::Unsupported` when unbundled, `cargo run` included; nothing shows until the user allows it |
| Dock badge | `NSDockTile` badge label (`1` to `9`, then `9+`) | none |
| Accessibility | `aqua-a11y-ax` (NSAccessibility through `accesskit_macos`), fed from the shared bridge | no screen-reader tree |
| Memory pressure | a dispatch memory-pressure source (warning, critical) driving the low-memory trim | none; the kernel only notifies processes above about 10 MB of footprint, which every real app is |
| Caches | an absolute `$XDG_CACHE_HOME/aqua` (else `~/Library/Caches/aqua`): font fallback index and `pipelines-metal.cache` | caches are not persisted |

Not provided on macOS: window materials other than solid fall back to an opaque
frame (no `NSVisualEffectView`), pinch, magnify and scroll phases are not input
events (momentum arrives as ordinary scroll events), `aqua::local_time` does not
exist, and setting the inner size of a full-screen window is `Unsupported`.

Tests never reach the user's session: `.cargo/config.toml` forces the named
test pasteboard (`AQUA_MAC_PASTEBOARD`) and the workspace cache directory, test
processes are unbundled so notifications are `Unsupported`, and
`aqua-verify/tests/isolation.rs` asserts all three on macOS. AppKit tests are
`harness = false` binaries with one child process per case, because AppKit
needs the process main thread.

## Validation

`cargo test -p aqua-platform`, strict workspace all-target Clippy and the
whole-workspace Windows GNU cross-check pass. Native Windows checks exercise
window lifecycle, monitor information, taskbar badges, mailbox wakeups and UIA
text geometry. Windows notification callback tests cover late activation,
replacement, withdrawal and destructor reentrancy without displaying toasts.
The full evidence and application migration boundary are recorded in
[EDITOR_READINESS.md](EDITOR_READINESS.md).
