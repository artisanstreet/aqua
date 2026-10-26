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
| `DialogRequest`, `FileFilter`, `dialogs_capability` | `dialogs` | Native file dialogs (rfd, portal-based on Linux — no GTK). |
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

## Explicitlyunsupported / deferred (honest)

- Typed in-app MIME drag-and-drop (only OS file drops via `FileDrop` +
  in-process `InProcessDnD` stub); notifications/taskbar/menus; global
  window coordinates on Wayland; macOS `NSVisualEffectView`; Linux
  compositor blur; software-rendering fallback policy; AT-SPI adapter
  milestone (tree vocabulary ready, adapter `cfg(windows)`); screen-reader
  end-to-end run (`end_to_end_verified: false`).

## Tests

58 unit tests, all headless, run in the SHARED workspace (no exclusions):
`scheduler` (18: pacing, merging, deadlines, fused gate+deadline wakeups,
backpressure sleep + starvation-freedom, overload recovery, resize storms,
remove-during-flight, present saturation, deadline supersede, multiwindow,
runtime target change); canonical key (`classify_key`: Char identity,
combining sequences, F-keys, repeat) / button / modifier / IME (multi-scalar
preserved) / touch-contact / theme / file-drop translation; focus+theme →
`surface_signal` material-recheck integration; material mapping +
transparency requirement + prefs-driven fallback (`select_material`);
clipboard backends; dialog builder + headless honesty; prefs overrides +
opaque-frame policy; a11y id packing, full role table, pure tree build,
action-data preservation, bridge wake-once-without-lock,
windowless handler dispatch; chrome seam defaults, stub honesty, 8 CPU
hit-test regressions (borders/corners, drag band + no-drag holes, reserved
strip exclusion, maximized/unresizable, explicit draggables).
`cargo test -p aqua-platform`: 58 passed.
`cargo check/clippy -p aqua-platform --all-targets`: clean, 0 warnings.
`cargo check -p aqua-platform --target x86_64-pc-windows-gnu --all-targets`:
clean (includes `windows.rs`, `prefs.rs` Windows probes, `a11y_winit`,
`chrome_winit` subclass + `windows_smoke` example). Example `windows_smoke` provided for the later
PowerShell native run (not run here).

## Relay note for parent → facade owner (cannot message from agent thread)

1. Additive-only this pass EXCEPT `Clipboard` trait is now fallible
   (`get/set/clear` → `Result`) — facade does not use it (verified by grep).
2. Adopt when ready: `translate_*_core` (modifiers now carried — resolves
   your `Modifiers::NONE` relay note), `surface_signal`, `ThemeChanged`,
   `ObservedWindowState::new`, `docs/SCHEDULER.md` completion wiring
   (`mark_presented` on next-acquire, not on submit).
3. Materials: set `transparent: true` + alpha surface + transparent clear
   or backdrops stay hidden; re-apply material on `surface_signal().material_recheck`.
4. a11y attach needs invisible-first windows (`with_visible(false)`) +
   a REQUIRED waker: `attach_with_waker(event_loop, window, waker)` where
   the waker wakes the loop (proxy) and routes to `drain_actions`. Without
   it AT actions stall while idle (fixed this pass; wake-once-without-lock
   unit-tested). AT clicks arrive as action requests with preserved
   payloads (`A11yActionData`) — never depend on mouse input for them.
5. `translate_key_event` kept its signature; logic moved to testable
   `classify_key` (winit `KeyEvent` has a private platform field and cannot
   be struct-built — facade mock code must not construct it literally).
