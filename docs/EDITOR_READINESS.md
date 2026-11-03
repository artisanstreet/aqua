# Beginning the Artisan Editor port

Status, 2026-11-03: **ready to begin the Windows conversation/composer port on
the current backend**. This is a starting-port gate, not production-switch
approval.

The readiness target is a Windows Editor port through Aqua's public application,
text, collection and platform APIs. Replacing the existing Editor remains an
application migration, with its own feature parity and release qualification.

## Required framework behavior

- Text editing follows the existing Editor's Blink-derived Windows rules for
  word movement, paragraph movement, selection extension and deletion. Shaped
  visual navigation preserves horizontal position across short and wrapped
  lines. Caret affinity distinguishes the two sides of soft wraps and bidi
  boundaries. Undo records preserve grapheme clusters and composition state.
- Pointer selection supports dragging, reversal, Shift-click, word and paragraph
  click units, word/paragraph drag extension, cancellation and edge autoscroll.
  A source document provides copy order and separators independently of mounted
  text leaves, including virtualized gaps. Source bindings validate byte ranges
  and the exact rendered source. Typed `Text::selection_source_in` declarations
  bind and recycle automatically. Explicit document IDs isolate panes, and
  validated copy substitutions handle decorative/alternate source text.
  Selection respects modal and disabled scopes.
- Native text clipboard commands copy, cut and paste through the application
  adapter. Failed clipboard writes preserve the original selection and text.
  AltGr remains available for text entry. Image and file clipboard payloads have
  explicit validated platform APIs.
- Rich static text carries inherited font families, weight, slant, size and
  inline sRGB colors through measurement, shaping and rendering. Editable fields
  support full font styling with plain source text. Selection uses source bytes.
- Nested opacity groups retain painter order and isolate overlapping children.
  Independent CPU compositing checks exercise actual GPU readback. Glyph colors
  retain item and ancestor opacity.
- Idle and selection-only frames reuse the base scene. Text changes compare
  exact source and reshape changed runs; font revisions invalidate old metrics.
  Scene reordering preserves live node identities.
- Windows accessibility exposes control roles and actions through the same
  bound field and button behavior as pointer and keyboard input. Native text
  ranges carry shaped grapheme geometry and byte selections. Initial snapshots
  are staged before visibility; inactive accessibility skips per-frame text
  metadata generation.

## Behavioral evidence

The tests use the real runtime pipeline and public consumer APIs:

| Contract | Evidence |
|---|---|
| Blink word rules, Unicode editing, dictionary boundaries | `crates/aqua-text/tests/navigation.rs`, `browser_editing.rs` |
| Shaped navigation, bidi, wraps and affinity | `crates/aqua-text/tests/browser_geometry.rs` |
| Exact source order and offscreen copy | `crates/aqua-text/tests/document.rs` |
| Pointer gestures, autoscroll, modal scopes and source lifecycle | `crates/aqua-runtime/tests/text-interaction.rs` |
| Transcript work counts and cache invalidation | `crates/aqua-runtime/tests/frame-work.rs` |
| Rich source measurement and restyling | `crates/aqua-runtime/tests/typography.rs` |
| GPU composition and colored text | `crates/aqua-render/tests/conformance.rs` |
| Transactional clipboard shortcuts | `crates/aqua/tests/clipboard.rs` |
| Native action translation and protected values | `crates/aqua/tests/accessibility.rs` |
| Combined virtualized conversation and composer | `crates/aqua/tests/ui_app/editor-workload.rs` |
| Windows message translation | `scripts/windows-input-smoke.ps1` |
| Native UI Automation behavior | `scripts/windows-accessibility.ps1` |

Final validation on 2026-11-03:

- `AQUA_REQUIRE_GPU=1 CARGO_BUILD_JOBS=2 bash scripts/validate.sh`: 13 passed,
  zero failed and zero skipped. This covers every workspace crate's tests,
  strict GPU execution, formatting, all-target Clippy with warnings denied,
  and the complete Windows GNU all-target check.
- `bash scripts/build-windows.sh todo dev`: rebuilt the final Windows app.
  `windows-accessibility.ps1` passed native roles, Unicode text retrieval,
  emoji-grapheme selection, value changes, invocation, bound state and close.
- Rebuilt `windows_input_smoke` in the Windows shell; the native message replay
  exited zero with `WINDOWS_INPUT_OK`.
- The new GPU conformance suite passed on Linux llvmpipe/Vulkan and native
  Windows AMD/DX12 (three tests per backend). The native run used
  `WGPU_BACKEND=dx12`, matching the application's graphics API.

## Architecture and performance limits

The working host still uses winit, wgpu, cosmic-text and glyphon. The native
D3D12/Win32/TSF backend replacement, full reference backend, render-thread
transport and DirectWrite pixel oracle in `ARCHITECTURE.md` remain separate
architecture work. Current GPU conformance is a tested subset of composition,
not a replacement for that complete reference backend.

Changed-content frames still walk the mounted UI tree and emit its base scene.
Virtualization bounds mounted rows; it does not remove dataset snapshots or
application-owned source text. Work counters prove specific skipped operations,
not superior latency, memory usage or performance compared with GPUI/Chromium.

Native UI Automation tests establish API behavior. They do not establish speech
quality in Narrator/NVDA, all assistive-technology behavior, or pixel equivalence
with Chromium. Native text patterns currently operate per text leaf; runtime
source selection spans leaves independently. Physical native IME qualification
remains necessary. Multi-click timing and distance currently use framework
defaults rather than the user's OS preference. Unicode dictionaries and data
versions can differ from Chromium, so bit-identical behavior is not claimed.

The Editor port still needs its Markdown renderer, media and rich
blocks, attachments, shortcuts, menus and settings connected to Aqua, followed
by interactive Windows qualification before switching the production Editor.
