# Window chrome (`Titlebar::Overlay`)

Native caption buttons over glass: the app draws the entire window —
including the title-bar area — while Windows draws the real minimize /
maximize / close buttons (with hover states, snap layouts, dragging,
double-click maximize, and resize edges). No custom-drawn fake buttons.
No `comctl32` dependency: the overlay chains the window proc through
`user32` (`SetWindowLongPtrW`/`CallWindowProcW`) with per-window state in
a window property, so apps need no activation manifest and no `build.rs`.

## Configuration

```rust
use aqua::{Config, Material, Theme, Titlebar, Window, px};

let config = Config::new().window(
    Window::new()
        .title("Tasks")
        .size(px(760.0), px(560.0))
        .theme(Theme::Dark)
        .material(Material::Glass)
        .titlebar(Titlebar::Overlay)
        .titlebar_height(px(40.0))
        .minimizable(true)
        .maximizable(true)
        .closable(true),
);
```

`Hidden` (or `decorations(false)`) is a plain borderless window with no
native buttons. `Native` is the standard OS frame. Legacy
`decorations(false)` takes precedence over the titlebar mode.

## Attaching

```rust
let window: winit::window::Window = /* created via host::window_attributes(&opts) */;
let chrome = WindowsChrome::install(&window, &opts)?;
// Keep `chrome` alive for the window's whole life. Dropping it detaches the
// overlay and restores the OS frame.
```

`install` keeps `decorations(true)` windows as-is (the `WS_CAPTION` style
is what keeps the buttons alive) and validates options first. On other
platforms `Overlay` returns `ChromeError::Unsupported` — fall back to
`Native`. Installing twice on one window returns
`ChromeError::AlreadyInstalled`; drop the old guard first.

`host::window_attributes(&opts)` maps the same fields to winit: `Hidden`
forces borderless, button flags map to enabled buttons. Transparency is
never forced: an opaque overlay keeps buttons, drag, snap, and resize fully
working — only the glass backdrop needs `transparent(true)` plus an alpha
surface cleared transparently.

## Drag regions

The top `titlebar_height` band hit-tests as caption (drag / double-click
maximize / window snap). Finer control — the runtime publishes these after
layout with the current scale (tuples are `(left, top, right, bottom)`,
client-local logical px, no reactive graph involved):

```rust
let regions = DragRegions {
    draggable: vec![(0.0, 40.0, 200.0, 80.0)],  // extra caption areas
    no_drag: vec![(8.0, 8.0, 200.0, 32.0)],      // text field stays clickable
};
chrome.set_drag_regions(&regions, scale)?;
// Tuple form with the same signature on stub and live guard:
chrome.set_drag_rects(&regions.draggable, &regions.no_drag, scale)?;
```

The tuple form `set_drag_rects(draggable, no_drag, scale)` takes the same
data. Non-drag holes always win over caption areas. Unknown regions stay
ordinary client input — never swallowed.

## Hit testing

`WM_NCHITTEST` goes to `DwmDefWindowProc` first, so natively drawn regions
resolve with OS hit codes (8/9/20 for the buttons). Only regions DWM
declines fall back to the built-in rules: resize borders/corners (skipped
when maximized or unresizable), the reported button strip reserved as
non-draggable input, then drag band and drag rects. Maximized windows clamp
to the monitor work area on every side including the top, so content never
sits under the taskbar.

## System menu

The drag band returns the native caption hit code and forwards unhandled
messages through the window procedure chain. Advanced integrations can
also call `chrome.show_system_menu(x_screen, y_screen)` for an explicit
menu gesture.

## Theme, DPI, focus

- `ThemeChanged` events re-resolve the title-bar theme
  (`apply_titlebar_theme_to_window`); focus loss re-checks the material
  (active/inactive backdrops differ).
- `WM_DPICHANGED` refreshes the drag band, glass margin, and DPI cache;
  per-monitor DPI resize borders use DPI-aware frame metrics.
- Preferences (`prefs::query_preferences`) drive solid fallbacks:
  `select_material` forces opaque when high contrast is on or transparency
  is off.

## Diagnostics

```rust
eprintln!("{}", chrome.diagnostic_string());
// aqua-chrome hwnd=0x... subclass=true dpi=144 buttons=Some(...) maximized=false
//   nccalc=3 nchittest=512 dwm=500 fallback=12
```

`diagnostics()` returns the same snapshot structurally (`ChromeStats`:
`nc_calc_size`, `nc_hit_test`, `dwm_handled`, `fallback_handled`,
`system_menus_shown`). A high `fallback_handled` share with a reported
button strip means DWM is declining hits that should be native —
investigate before release.

## Known gaps

- Native caption button placement and hit testing are delegated to DWM;
  right-to-left behavior has not been tested on a native host.
- Auto-hide taskbars report a full-monitor work area, so a few maximized
  pixels may sit under an auto-hidden bar.
- When DWM does not report button bounds, only its own hit-test responses
  can identify caption buttons. The remaining top band stays draggable.
- Visual glass (acrylic/mica behind the band) needs a real Windows 11
  host to verify; compile checks cannot prove pixels.
