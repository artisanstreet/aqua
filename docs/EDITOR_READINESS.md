# Beginning the Artisan Editor port

Status note (2026-11-08): the qualification results below belong to their dated
implementation. Several listed `aqua-render` and `aqua-platform` paths were
replaced by the native graph/transport/D3D11/Win32 crates. They are historical
references, not commands that can be rerun against the present tree. Current
architecture and remaining limitations are recorded in `../ARCHITECTURE.md`.
A passing historical qualification does not prove the current dirty worktree
passed every former gate.


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
