# Beginning the Artisan Editor port

Status note (2026-11-08): the qualification results below belong to their dated
implementation. Several listed `aqua-render` and `aqua-platform` paths were
replaced by the native graph/transport/D3D11/Win32 crates. They are historical
references, not commands that can be rerun against the present tree. Current
architecture and remaining limitations are recorded in `../ARCHITECTURE.md`.
A passing historical qualification does not prove the current dirty worktree
passed every former gate.


## Linux status (2026-11-21)

Aqua runs on Linux with Wayland and Vulkan (`../ARCHITECTURE.md` record 14.LW,
M1 to M12; service table in [PLATFORM.md](PLATFORM.md)). The Editor port to
Linux (M13) needs to know:

- Ship `libdxcompiler.so` (pinned DXC v1.9.2609) beside the executable or in
  `../lib`, or the sky and any other custom pass report `Unsupported`.
- Set `AquaConfig::app_id` to the reverse-DNS id of the installed `.desktop`
  file. The compositor groups windows by it, and the launcher badge and portal
  notifications key on it; without it the badge returns `BadgeError::Unsupported`.
- Fonts come from fontconfig at run time (`libfontconfig.so.1`). Families the
  Editor names explicitly still win; generic families and fallback follow the
  user's fontconfig aliases. `AQUA_FONT_DIRS` is not needed.
- Text fields built on Aqua's editable text set the primary selection and paste
  it on middle click without Editor code. A custom text surface that wants the
  same behaviour sends `ShellCommand::PrimarySelectionWrite` and
  `PrimarySelectionRead`; on Windows both do nothing.
- `aqua::prelude::file_icon` returns `None` on Linux, and `aqua::local_time`
  does not exist there. The Editor's only `file_icon` call (default-browser icon
  in `editor-aqua/src/shell/browser.rs`) already has no Linux browser to show, and
  it formats times with its own module.
- Opening URLs and files and showing file dialogs pass the window's exported
  handle and an activation token to the portal, so the opened app may take focus
  and dialogs stay on top of the Editor.
- Window position, always-on-top, lowering, restoring from minimized and raw
  mouse input are unsupported on Wayland and come back as `Unsupported` with a
  reason; materials other than solid fall back to an opaque frame.
- Caches live in `$XDG_CACHE_HOME/aqua` (else `~/.cache/aqua`).
- Editor tests that start Aqua apps on Linux should copy Aqua's
  `.cargo/config.toml` `[env]` dead ends and run against a private headless
  sway and D-Bus, as `aqua/tests/host_wayland.rs` does, so they never reach the
  developer's desktop.

## macOS status (2026-11-22)

Aqua runs on macOS 14.0 or newer on Apple Silicon with AppKit and Metal
(`../ARCHITECTURE.md` record 14.MA, M0 to M14; service table in
[PLATFORM.md](PLATFORM.md)). Everything was proven on GitHub `macos-15`
runners, not on a physical Mac. The Editor port to macOS (M15) needs to know:

- Ship as an app bundle made by `scripts/macos-bundle.sh` (or an equivalent):
  `Info.plist` with `CFBundleIdentifier` equal to `AquaConfig::app_id`,
  `LSMinimumSystemVersion` 14.0 and `NSHighResolutionCapable`, and
  `libdxcompiler.dylib` (thinned to arm64) and `libspirv-cross-c-shared.dylib`
  in `Contents/Frameworks`. Without the two libraries the sky and every other
  custom pass report `Unsupported`; no environment variable is needed when they
  are bundled. The CI leg bundles `examples/macos_bundle.rs` and runs it from
  the bundle to prove both.
- Notifications work only from that bundle. Unbundled, `cargo run` included,
  `NotificationCenter::new` returns `NotificationError::Unsupported`; a bundled
  app is `Denied` until the user allows it. The bundle is signed ad hoc;
  Developer ID signing, the hardened runtime and notarization are release work.
- Use `Titlebar::Overlay`: the traffic lights are native and centred in
  `titlebar_height`, so Editor-drawn caption buttons must be hidden on macOS.
- Move Editor shortcuts to `aqua::keys::primary()`, so Ctrl on Windows and Linux
  is Command on macOS. Text fields already follow Chrome on macOS (Option and
  Command arrows, Command-A, -C, -V, -Z); word boundaries stay Blink's.
- Aqua installs a standard app menu (About, Hide, Hide Others, Show All, Quit)
  and an Edit menu, and turns Quit and Command-Q into a close request for every
  window, which `can_close` may veto. Editor-specific menu items still need
  adding.
- `file_icon` works through `NSWorkspace`. `aqua::local_time` does not exist; the
  Editor formats times itself.
- Fonts come from Core Text; leave `AQUA_FONT_DIRS` unset. Caches live in
  `~/Library/Caches/aqua` unless an absolute `XDG_CACHE_HOME` is set.
- Materials other than solid fall back to an opaque frame (no
  `NSVisualEffectView`), and pinch, magnify and scroll phases are not input
  events; trackpad momentum arrives as ordinary scroll events.
- Editor tests that start Aqua apps on macOS must be `harness = false` binaries
  that run AppKit on the main thread with one child process per case, as
  `aqua/tests/host_macos.rs` does, and should copy Aqua's `.cargo/config.toml`
  `[env]` entries so they use the named test pasteboard.
- A `macos-15` CI leg like Aqua's needs the pinned LunarG SDK for DXC and
  SPIRV-Cross, `AQUA_REQUIRE_*` flags, and `activate(ignoringOtherApps:)`
  semantics (Aqua already activates on launch).

Not verifiable without a physical Mac, and therefore release qualification for
the Editor: Retina rendering (the runner is 1x), ProMotion 120 Hz pacing (the
runner reports no presentation times), real VoiceOver speech and rotor
navigation, trackpad momentum and gesture feel, notification banners and
actions, candidate windows of real input methods beyond Kotoeri's Romaji mode,
appearance and reduce-motion changes made in System Settings, and Developer ID
signing with notarization.

Status, 2026-11-03: **ready to begin porting the entire Windows Editor app**. The target is
starting the Windows port of the entire Editor through public Aqua APIs. It covers
the shell and every Editor section, including rich blocks and native services.
Replacing the production Editor remains a separate application migration and
release decision.

The inventory follows the actual GPUI consumers in `editor`, `editor-core`,
`editor-ui`, `editor-composer`, `editor-conversation`, `editor-sidebar`,
`editor-settings`, `editor-marketplace`, `editor-inspector`, and `editor-sources`.
Editor audio, HTTP, storage, and provider engines retain their application-owned
backends; typed worker delivery connects their results to Aqua's UI thread.

## Required contracts and evidence

| Editor surface or dependency | Aqua contract | Behavioral evidence |
|---|---|---|
| Shell, home, sidebar, inspector | Bound state, action propagation, scoped shortcuts, pointer capture, hover/cursors, responsive layout, focus indicators | `aqua-ui/tests/input.rs`, `aqua-runtime/tests/routed-input.rs`, `aqua/tests/ui_app/editor-shell.rs` |
| Settings | Stepped sliders, captured resizing, select options and disabled choices, roving tabs, numeric accessibility, live per-window frame pacing and vsync, text raster modes | `aqua-runtime/tests/runtime/controls.rs`, `aqua-runtime/tests/scheduler.rs`, `aqua-render/tests/vector/composition.rs` |
| Marketplace and sources | Virtual lists/tables/trees, source actions, stable row identity, nested scrolling with edge chaining | `aqua/tests/collections_conformance.rs`, `aqua-ui/tests/input.rs`, `aqua/tests/ui_app/editor-shell.rs` |
| Palette, menus, tooltips and dialogs | Bound Element portals, reactive filtered results, independent popup scopes, modal isolation, light dismiss, focus restoration, timed tooltips | `aqua-runtime/tests/runtime/controls.rs`, `aqua/tests/ui_app/editor-shell.rs` |
| Conversation and composer | Source-backed selection across mounted and virtualized content, variable heights, reading anchors, streaming text, bound editor state | `aqua/tests/ui_app/editor-workload.rs`, `aqua-runtime/tests/text-interaction.rs` |
| Browser-style text behavior | Blink-derived Windows word/paragraph keys, visual caret affinity, graphemes and dictionaries, multi-click gestures, reversal and autoscroll | `aqua-text/tests/navigation.rs`, `browser_editing.rs`, `browser_geometry.rs`, `aqua-runtime/tests/text-interaction.rs` |
| Native text and focus | Structured text/image/file paste after routed interception, composition cancellation on blur, exact drop coordinates, transactional autocomplete and undo, shaped IME anchor geometry | `aqua/tests/clipboard.rs`, `aqua-runtime/tests/routed-input.rs`, `aqua/tests/windows_text.rs` |
| Rich text and links | Font inheritance, inline colors and fixed-width mention cells, decoration geometry, range activation with modifiers and drag suppression, transformed geometry | `aqua-text/tests/decorations.rs`, `aqua-runtime/tests/typography.rs`, `routed-input.rs` |
| Charts, diagrams, icons and themed chrome | Paths, SVG arcs, holes, stroke styles, multi-stop gradients, tinted/full-color SVG, rounded ancestor clips and painter order | `aqua-render/tests/vector/raster.rs`, `vector/composition.rs` |
| Previews and attachments | Managed leased image assets, bounded asynchronous decode and GPU upload, animated image timelines, contain/cover/crop, content blur, subtree transforms | `aqua/tests/images/lifecycle.rs`, `images/composition.rs`, `aqua-render/tests/media.rs`, `vector/composition.rs` |
| Theme changes and streaming highlights | Real captured-frame transition and alpha-preserving text recoloring; native DirectWrite and unhinted outline raster modes | `aqua-render/tests/vector/composition.rs`, `aqua/tests/windows_desktop.rs` |
| Custom model/media renderers | Shared-device offscreen passes and registered external surfaces, including explicit removal | `aqua-render/tests/headless.rs`, `aqua/tests/images/composition.rs` |
| Background results and window lifecycle | Bounded mailboxes, wake without polling, stale-window protection, close vetoes, native commands, monitor work areas | `aqua/tests/mailbox.rs`, `application_services.rs`, `windows_desktop.rs`, `aqua-platform/tests/monitor.rs` |
| External actions and notifications | Validated URL/path launch, parented asynchronous dialogs, native toast actions/withdrawal, taskbar badge and Explorer-restart replay | `aqua-platform/tests/shell.rs`, `dialogs.rs`, `notifications.rs`, `notifications/native.rs`, `badge.rs`, `aqua/tests/windows_desktop.rs` |
| Accessibility | Native roles/actions and range values, real TextPattern grapheme geometry and selection, transformed physical bounds, lazy snapshots | `aqua/tests/accessibility.rs`, `aqua-platform/tests/access-tree.rs`, `aqua/tests/windows_text.rs`, `scripts/windows-accessibility.ps1` |

The `editor_shell` example is a functional framework consumer spanning sidebar,
conversation/composer, settings, marketplace, sources, inspector, palette,
managed images, SVGs and vector charts. `scripts/windows-editor-shell.ps1`
drives its native interactions. It is an integration fixture; application logic
and Editor-specific rendering are still implemented in the Editor repository.

## Qualification gate

The final implementation passed:

- `AQUA_REQUIRE_GPU=1 CARGO_BUILD_JOBS=2 bash scripts/validate.sh`: 13 gates,
  zero failures and zero skips, including strict all-target Clippy and the
  complete Windows cross-check.
- Actual native Windows full-shell UI Automation and pointer interactions,
  plus transformed TextPattern geometry and selection checks.
- Actual native desktop mailbox wake, resize, taskbar badges, two-window
  lifecycle, close veto, live presentation settings, both text raster modes
  and snapshot animation scheduling without worker polling.
- Native Windows AMD DX12: 45 renderer unit tests, 13 rendering regressions
  and 10 managed-image tests. These cover depth-tested meshes, compute,
  composition, clipping, vectors, images, blur, snapshot transitions, text
  maps, native raster modes and text-buffer lifetime across ordered chunks.
- The full Linux renderer suite: 82 tests. Native notification callback
  tests also passed for replacement, withdrawal and single-use activation.

The final lifecycle review additionally verified removal clears retained
pixels, resize repaints an unchanged scene, image replacement respects the
cache budget, shared asset registries wake every subscribed window, and
invisible animations remain idle.

## Limits and application migration

The host uses winit and wgpu; shaping uses cosmic-text and glyphon. Windows
PlatformDefault glyph masks use DirectWrite, while Outline uses unhinted
outlines; color glyphs retain their color-font raster path. This does not complete
the separate native host rewrite, render-thread transport or full reference
backend described in `ARCHITECTURE.md`.

Changed-content frames still walk the mounted UI tree and emit its base scene.
Virtualization bounds mounted rows; it does not remove application-owned source
text or dataset snapshots. Work counters establish skipped operations, not a
latency or memory advantage over GPUI or Chromium.

Native UI Automation checks establish API behavior, not Narrator/NVDA speech
quality or pixel equality with Chromium. TextPattern ranges operate per text
leaf; runtime source selection spans leaves independently. The validation host
has only a Norwegian keyboard and no composition IME installed. Physical CJK
candidate-window qualification remains a release check; no language settings
were changed. Unicode dictionary versions can differ from Chromium.

The actual migration still connects Editor's models, Markdown/rich-block
renderers, mention overlays, commands, network/media ownership and persistence
to these contracts, then qualifies feature parity before switching the shipping
Editor. The framework gate covers the whole application rather than limiting
the starting port to conversation/composer.
