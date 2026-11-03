# Aqua platform adapter (`aqua-platform`)

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

Global window coordinates on Wayland, macOS `NSVisualEffectView`, Linux
compositor blur, a software-rendering fallback policy and an AT-SPI adapter
remain outside the Windows qualification. Native UIA tests establish API
behavior; screen-reader speech quality and physical CJK candidate-window
placement remain release qualification tasks.

## Validation

`cargo test -p aqua-platform`, strict workspace all-target Clippy and the
whole-workspace Windows GNU cross-check pass. Native Windows checks exercise
window lifecycle, monitor information, taskbar badges, mailbox wakeups and UIA
text geometry. Windows notification callback tests cover late activation,
replacement, withdrawal and destructor reentrancy without displaying toasts.
The full evidence and application migration boundary are recorded in
[EDITOR_READINESS.md](EDITOR_READINESS.md).
