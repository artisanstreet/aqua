# `aqua-text` — shared text infrastructure

Owner: text agent. Crate: `crates/aqua-text/`. API version: **1**
(`aqua_text::AQUA_TEXT_API_VERSION`).

Real implementation over mature shaping crates, not a stub. No window,
GPU, or platform dependency — everything runs headless and is covered by
`cargo test -p aqua-text` (34 tests: shaping, bidi, emoji/combining,
multiline, wrapping, selection, IME, masking, undo, shared cache).

## Pinned versions (agreed with renderer)

| Crate | Version | Where |
|---|---|---|
| `cosmic-text` | **0.19** | `aqua-text` + `aqua-render` (same shaping) |
| `glyphon` | **0.12** | `aqua-render` only (rasterization) |
| `wgpu` | **30** | `aqua-render` only |
| `fontdb` | 0.23 | transitive via `cosmic-text 0.19`; direct dep for `fontdb::ID`/DB access |
| `unicode-segmentation` | 1.12 | grapheme/word boundaries in the editing model |

Asserted in code: `aqua_text::{COMPAT_COSMIC_TEXT, COMPAT_GLYPHON, COMPAT_WGPU}`.
Bump together with `aqua-render`; any bump invalidates the contract below.

## One font system, shared

```rust
let fonts = aqua_text::SharedFonts::new(); // Arc<Mutex<FontManager>>, Send+Sync
```

- `FontManager::new()` loads system fonts; `new_without_system_fonts()`
  starts empty (hermetic tests, custom font sets).
- `load_font_bytes(Vec<u8>)`, `load_font_file(path)`, `face_count()`,
  `has_family()`, `family_names()`, `export_face_data(fontdb::ID)`.
- `font_system()` / `font_system_mut()` expose the shaping engine state.

**Renderer contract (resolves the duplicate-database problem):**
glyphon's `TextRenderer::prepare` borrows `&mut FontSystem` from the
caller, so `aqua-render` must NOT create its own `FontSystem`. Pass
`fonts.lock().font_system_mut()` together with `TextLayout::buffer()`
(the retained shaped buffer). Same buffer + same font system ⇒
measurement and pixels cannot disagree. Custom fonts loaded into the
shared manager are visible to both sides automatically.

## Layout: `TextLayout` → `Arc<LaidOut>`

`RunRegistry` mints the contract-§4 `TextRun` handles: `insert(layout) ->
(RunId, revision)`, `update` (bumps revision), `remove` (unmount),
`resolve(id) -> Option<RunHandle { revision, layout }>` for the renderer.
`RunId.0` + revision map 1:1 onto core's `TextRun { id, revision }` when
it lands; no duplicate struct is defined here. Stale ids fail closed
(`None`). One retained shaping `Buffer` per live run — UI removes runs on
unmount (see Gaps: no hidden eviction).

```rust
let mut t = TextLayout::new();
t.set_text("hello", &style);      // or set_spans(vec![StyledSpan{..}]) for inline styling
t.set_line_mode(LineMode::Multi); // Single folds '\n' to space (1B→1B, indices preserved)
t.set_wrap(WrapMode::WordOrGlyph);
t.set_align(TextAlign::Start);
t.set_max_width(Some(240.0));     // logical px, unscaled
t.set_scale(1.5);                 // device/pinch scale; outputs are scaled px
t.set_mask(Some('•'));            // optional password masking, source coords preserved
let laid: Arc<LaidOut> = t.layout(&mut fonts.lock());
```

`LaidOut { size: (w, h), constrained_width, lines, first_baseline_y }`.
`LaidLine { paragraph, display_range, y_top, height, baseline_y, width,
rtl, glyphs }`.
`LaidGlyph { src_byte_start/end, font_id: fontdb::ID, glyph_id: u16,
font_size_px, x, y, w, bidi_level }` — glyph `x` has line alignment baked
in; line boxes are the contract for carets/selection, glyph data for
custom canvas work and accessibility.

Geometry (all source coordinates unless noted):

- `layout.caret(byte) -> Option<CaretRect { x, y_top, height }>`
- `layout.hit(x, y) -> Option<usize>` (grapheme-snapped)
- `layout.selection_rects(range) -> Vec<Rect>`
- `layout.buffer() -> Option<&cosmic_text::Buffer>` — renderer path, see above
- `measure_text(&mut fonts, text, &style, max_width, scale) -> (w, h)` —
  one-shot helper for `aqua-ui` Taffy measure callbacks (returns unscaled
  logical px). Retained `TextLayout` for repeated measurement.

Caching: per-instance dirty tracking (unchanged inputs skip shaping) plus
`SharedLayoutCache::new(capacity)` LRU sharing `Arc<LaidOut>` across
widgets — `layout_shared(&mut fonts, &mut cache)`. Capacity counts entries
(8–128 typical); hits/misses/evictions counters exposed. Font loads
bump a revision proxy in the cache key, so newly installed fonts reshape.

Bidi note: shaping, fallback and paragraph direction come from the engine
(full UAX #9/#29 via `harfrust`/`unicode-bidi`, not reimplemented).
`hit()` is exact for LTR/RTL paragraphs and nearest-visual-edge inside
mixed-direction runs (documented in code; still a valid boundary).

## Editing: `TextEditor`

```rust
let mut ed = TextEditor::with_text("hi", LineMode::Single);
ed.insert("!"); ed.move_word_left(true); ed.delete_backward();
ed.set_composition("にほん", 0..9); ed.commit_composition(); // or cancel_composition()
ed.undo(); ed.redo(); // bounded (default 100, set_undo_limit), adjacent typing merges
```

- All positions are byte indices on grapheme boundaries; movement/deletion
  are cluster-safe (combining marks, ZWJ emoji tested).
- `set_caret(pos)` / `set_selection(sel)`: hit-test placement. Both clamp
  into the text and snap to the nearest grapheme boundary (ties snap down);
  anchor/caret order is preserved exactly as given, so shift-extend state
  round-trips. Both cancel an active preedit (displaced text restored
  losslessly) and never touch undo history. UiApp loop is closed:
  `layout.hit(x, y)` -> `editor.set_caret(byte)` -> `layout.caret(...)`.
- Replace (typing or IME commit over a selection) is ONE undo step restoring
  content and selection; mode change to single-line drops history only when
  it folds newlines; IME cancel restores displaced selections losslessly.
- Single-line strips `\n`/`\r` on input; multi-line honors them.
- IME: `display_text()` (committed + preedit, what gets laid out),
  `composition_range()` (display coords, candidate-window placement),
  `ime_caret()` (preedit cursor in display coords). Commit is one undo
  step; cancel drops the preedit; keystrokes replace it silently.
- Platform wiring: feed winit `Ime::Preedit/Commit` events into
  `set_composition/commit_composition`; position the OS candidate window
  with `layout.caret(ed.ime_caret())`. Clipboard/cut/copy stay in
  `aqua-platform` using `selected_text()`/`delete_selection()`/`insert()`.

## Gaps (not claimed)

- No GPU rasterization here — renderer side (glyphon).
- No line breaking inside `TextEditor` movement (up/down, page) — needs
  layout lines; UI composes `layout` + `hit` for that.
- No spellcheck, hyphenation dictionaries, or advanced typography UI
  (ligature toggles beyond `letter_spacing_em`); shaper supports them,
  no API yet.
- `fonts_revision` is an explicit monotonic counter bumped by every
  `load_font_bytes` / `load_font_file` (face count alone would miss
  same-count font replacement). Loading through `font_system_mut().db_mut()`
  bypasses it — documented, use `load_*` instead.
- Accessibility tree export is `aqua-ui`'s job using `LaidOut` runs.
- `TextLayout::layout` requires at least one loaded font: an empty
  `FontManager` panics inside the shaper (cosmic-text 0.19 `no default font
  found`). UI must always lay out against system fonts or after loading at
  least one face; the signature stays infallible by contract.

## Status for peers

- **Renderer**: `TextLayout::buffer()` + shared `FontSystem` ready; build
  `glyphon::TextArea { buffer, … }` from it. If you defined
  `TextRun{id, revision}`, map it to a retained `TextLayout` keyed by that
  id — propose the shape in the contract and I will add a convenience
  `TextRun → layout` registry if wanted.
- **UI**: `measure_text` + retained `TextLayout` ready for Taffy callbacks
  before tree integration; `Text(editable:true)` = `TextEditor` +
  `TextLayout` on the same source coordinates (see
  `tests/editing_layout.rs`).
- **Platform**: IME needs are `composition_range()` + `ime_caret()` →
  `layout.caret()`; no native calls from this crate.

## FROZEN renderer API (text agent, 2026-10-25 — no further trait churn)

`crates/aqua-render/src/text.rs` is final. Versions: cosmic-text 0.19,
glyphon 0.12, wgpu 30. Manifest requirement (already applied):
`aqua-text = { path = "../aqua-text" }` in `aqua-render/Cargo.toml`.

```rust
// Shared handles — construct once (facade), clone into renderer + UI.
pub struct TextBackend { pub fonts: SharedFonts, pub runs: SharedRuns }
impl TextBackend {
    pub fn new(fonts: SharedFonts, runs: SharedRuns) -> Self;
    pub fn with_system_fonts() -> Self; // tests / single-owner hosts
}

// Painter-order queue item. No strings: the registry owns text.
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub id: u64,                    // TextRun.id
    pub revision: u64,              // TextRun.revision (scene-committed)
    pub at_physical: [f32; 2],      // top-left, physical px
    pub color_glyphon: glyphon::Color, // sRGB u8, alpha ALREADY × opacity
    pub clip_physical: Option<[f32; 4]>, // [x,y,w,h] physical px, None = unclipped
}

// Same-encoder context. Renderer keeps owning the encoder; text appends
// ONE Load pass in painter order. No submit inside.
pub struct TextFrame<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub target: &'a wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TextStats {
    pub areas: usize,                  // resolved + prepared
    pub stale_skipped: u64,            // unknown ids (explicit, counted)
    pub revision_mismatches: u64,      // STALE scene revision: SKIPPED, counted
    pub atlas_upload_bytes: Option<u64>, // ALWAYS None: glyphon exposes no counter
}

pub trait TextProvider: Send + Sync {
    fn queue_run(&mut self, run: ShapedRun);
    fn encode_queued(&mut self, frame: TextFrame<'_>) -> TextStats;
    fn measure_ink(&mut self, id: u64) -> Option<Rect>; // logical ink at origin
    fn trim_atlas(&mut self);   // ONCE after final submit, never mid-frame
    fn stats_last(&self) -> TextStats;
}

impl GlyphonProvider {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue,
               format: wgpu::TextureFormat, backend: TextBackend) -> Self;
    pub fn backend(&self) -> &TextBackend;
    pub fn set_format(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
                      format: wgpu::TextureFormat);
}

// No shaping, no fallback strings: placement/color/opacity/clip mapping.
pub fn resolve_scene_text(run: &TextRun, at_logical: [f32; 2], color: Oklch,
    opacity: f32, clip_logical: Option<Rect>, scale: f32) -> ShapedRun;
```

Color rule (verified vs glyphon 0.12 source): atlas is `ColorMode::Accurate`,
so `default_color` bytes are sRGB and linearized in-shader. Pass
`Oklch::to_srgb8` with alpha pre-multiplied by opacity — linear bytes would
double-linearize (white primaries hide the bug; mid-gray exposes it).
Render target stays the linear working texture, matching `to_gpu()` rects.
`tests/headless.rs::text_color_matches_rect_same_oklch` locks this in.

Coherence rule (strict): a scene revision older than the registry revision
is SKIPPED and counted — the current layout is NEVER painted under a stale
`TextRun` revision, since old scene geometry/state plus new text would be an
incoherent commit. The single decision point is `resolve_for_encode()`,
which returns `Ok(handle)` only for the live revision (`Err(SkipReason)` —
carrying no layout — otherwise), so the encoder has nothing to paint for
mismatches. The host must repair by re-committing the scene run at the live
revision; UI writes the registry BEFORE the scene, so the next consistent
commit paints. Treat `revision_mismatches > 0` as a re-render request.
`text.rs` unit test `stale_revision_never_yields_current_layout` and GPU
test `stale_revision_paints_nothing_and_counts_mismatch` lock this in.

## Caller patch instructions (renderer owner — apply to your mirror)

`renderer.rs` is yours; the text agent will not edit it again. Against
your trusted mirror (which has `queue_run` + `flush` callsites), apply:

1. Import: `use crate::text::{resolve_scene_text, GlyphonProvider,
   ShapedRun, TextBackend, TextFrame, TextProvider};`
2. Construction: add `Renderer::new_with_text(device, queue, format,
   backend: TextBackend)` (delegating constructor for the shared backend);
   keep `new()` delegating with `TextBackend::with_system_fonts()`.
   Provider line becomes
   `Box::new(GlyphonProvider::new(&device, &queue, format, backend))`.
3. `prepare()` damage arm for `DisplayItem::Text`: replace the
   `format!("run#{}")` measure with `self.text.measure_ink(run.id)`.
   `Some(ink)` → existing `record_text_change` flow. `None` → repaint any
   recorded `text_ink.remove(&run.id)` via `record_rect_change`, then
   `record_change(item, None)` (conservative full).
4. `prepare()` painter walk `DisplayItem::Text` arm: derive the
   logical clip from `clip_stack` (physical ÷ scale; `w < 0` → `None`)
   and call `resolve_scene_text(run, *at, *color, *opacity, clip_logical, s)`
   (opacity and clip now honored; no `let _ = opacity`).
5. `Op::Texts` arm in `draw()`: DELETE the submit-boundary
   (`queue.submit([encoder.finish()])` before text and the encoder
   re-creation after). Queue runs, build
   `TextFrame { device: &device, queue: &queue, encoder: &mut encoder,
   target: &base_view, width: self.width, height: self.height }`, call
   `encode_queued`. Do NOT add `stats.atlas_upload_bytes` (None) to byte
   totals; the three `RenderDiagnostics` text counters
   (`text_areas_last`, `text_stale_last`, `text_mismatch_last` — already
   added to `resource.rs`) take the exact counts.
6. `draw_group_isolated()`: single encoder — rect pass, then (if any
   group texts) queue + `encode_queued` into the SAME encoder targeting
   the scratch view, then ONE submit. Group texts keep full color;
   composite applies opacity once.
7. End of `draw()`, after the final `queue.submit`: call
   `self.text.trim_atlas()`. Never trim between encode and submit.
8. Invariants to keep: one `prepare` per `encode_queued` per target,
   immediately followed by its render (no prepare(A)+prepare(B) before
   render(A) — glyphon's instance buffer would alias); locks over
   fonts/registry are held only inside `encode_queued`/`measure_ink`.

## Follow-ups owned elsewhere (not text)

- Render/graphics: `Renderer::new(format)` vs surface sRGB format mismatch
  (headless base is linear Rgba8Unorm; surface path must normalize the
  working format) and `frames_presented` counting offscreen draws instead
  of actual presents — both need the produced-vs-presented split.
- Render diagnostics: surface the new text counters in the inspector.
