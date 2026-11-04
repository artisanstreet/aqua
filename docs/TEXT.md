# `aqua-text` — shared text infrastructure

Owner: text agent. Crates: `aqua-text`, `aqua-fonts-system`,
`aqua-oracle-dwrite`. API version: **1** (`aqua_text::AQUA_TEXT_API_VERSION`).
The stack is specified in `ARCHITECTURE.md` section 8; this file documents the
crate surface and the measured behaviour. Windows only.

`aqua-text` has no window, GPU or platform dependency. It depends on
HarfRust (shaping), skrifa (outlines, color, bitmap strikes), zeno (exact-area
coverage), `unicode-bidi`, `icu_segmenter` and `unicode-segmentation`. It does
not depend on cosmic-text, glyphon, wgpu or fontdb. Tests that need real fonts
(`FontManager::new()`) run on Windows; the pure stages (breaking, tone, slug,
resampling, font matching) also run under `aqua-cargo test`.

## Pipeline

| Stage | Module | Notes |
|---|---|---|
| Discovery | `aqua-fonts-system` | Scans `%SystemRoot%\Fonts` and `%LOCALAPPDATA%\Microsoft\Windows\Fonts` for `.ttf/.otf/.ttc/.otc`, memory-maps each file once (the only `unsafe` in the text stack, in `memmap2::Mmap::map`) and reads family names (typographic and legacy, English first), PostScript name, weight, stretch and style from `name`, `OS/2` and `post` through skrifa. Matching is CSS font matching (stretch, then style order, then the weight search order). No fontdb. |
| Shaping | `shape.rs` | HarfRust per run; spans split at font, bidi level and script; `ShapedParagraph` is the tier-1 cache. Synthetic bold widens each non-zero advance by the stroke width, `embolden_width(size)` = em/24, as FreeType does. |
| Breaking | `lines.rs` | Tier 2: wrap points per width; never re-shapes (`reshape_count`). |
| Rasterization | `raster.rs`, `strike.rs` | See below. |
| Slug outlines | `slug.rs` | See below. |
| Residency | `residency.rs` | `GlyphResolver` interns faces to the `GlyphKey` of `aqua-media` and fills `Residency` (mask atlas and RGBA atlas). |
| Tone | `aqua_graph::tone` | Microsoft grayscale contrast and gamma model; `linear_blend_alpha_over` takes the real backdrop luminance. |

## Rasterization

- Outline glyphs: unhinted skrifa outlines, exact-area coverage, 4 horizontal subpixel bins, y snapped, size quantized to 1/64 px, synthetic italic skew, fake bold (stroke unioned with the fill) when the requested weight is 600 or more and the face is lighter and not variable.
- COLR v0/v1 color glyphs are painted on the CPU into a premultiplied RGBA bitmap.
- Bitmap strikes (CBDT, sbix, and EBDT masks) go through `strike.rs`. The strike nearest the requested size is decoded (PNG, premultiplied BGRA, or 1/2/4/8-bit masks through skrifa's decoder), placed from its bearings, and resampled to the requested size: area-averaged down, bilinear up. The subpixel bin shifts the resampling phase, so the four bins of a bitmap glyph are four distinct, ink-conserving bitmaps and a bitmap glyph moves exactly like an outline glyph.
- EBDT mask strikes are used only when the font has them: always when the glyph has no outline, and otherwise only when the strike ppem equals the requested size within half a pixel (a hinted bitmap strike at its design size, as GDI does). Elsewhere the outline is used.

## Slug outlines for scale animation

`slug::SlugGlyph` is a size-independent, em-normalized outline: quadratic
Béziers (cubics are subdivided to 1/8192 em; lines become degenerate
quadratics), plus horizontal and vertical band tables. Each band lists the
curves overlapping it, sorted by descending far-side extent so a shader can
stop at the first curve that cannot reach the pixel. `texels()` packs the
curves as two RGBA32F texels each (`p1.xy p2.xy`, `p3.xy 0 0`); `BandSet`
exposes the ranges and index lists for upload as a second buffer.
`SlugGlyph::coverage(x, y, pixels_per_em)` is the CPU reference of the pixel
shader (dual-axis root classification with the `0x2E74` table, weighted by the
distance of the nearest crossing), and is what the tests compare against the
exact-area rasterizer.

`SlugCache::lookup(fonts, key)` returns `Outline`, `Blank` (spaces) or
`Unsupported` (color glyphs and synthetic bold, which stay on the atlas path).
The cache is keyed by face and glyph only, so scale animation never builds a
new entry per frame, and is flushed when the font revision changes.
Measured on Windows against the rasterizer for `o g S @ W &` at 12, 17, 32 and
77 px: mean coverage error below 0.045 (the worst case is small `g`), ink
ratio within 5%; italic `H` at 24 and 64 px within 3%. The GPU side (curve and
band buffers, the pixel shader, selection of Slug over the atlas while a
text node has an animated scale) is not built yet.

## Tone and the backdrop

The pipeline blends in linear light and DirectWrite blends in sRGB.
`ToneCurve::linear_blend_alpha` remaps the toned coverage so the linear blend
lands on DirectWrite's result when the backdrop is the contrasting extreme
(black behind light text, white behind dark text). For other backdrops the
result drifts (gray 200 on gray 30 measures 0.88 to 0.96 of DirectWrite's ink).
`linear_blend_alpha_over(coverage, text, backdrop)` is exact for any backdrop
(tested to 1/255 over a text by backdrop by coverage grid). The glyph shader
still uses the contrasting-backdrop form; using the exact form needs the
backdrop luminance, either as a per-run constant from the compositor (the
background of the surface the text sits on) or by reading the render target in
the glyph pass.

## Real-font verification (Windows)

- CBDT: Noto Color Emoji 2.042 (the Debian `fonts-noto-color-emoji` package; `CBDT` and `CBLC`, 109 ppem strike) at 16, 24, 32, 48 and 96 px and all four subpixel bins: bitmap width equals `size * 136 / 109` within one pixel, ink across bins within 2%. Run with `AQUA_CBDT_FONT` set to the font file (`tests/real_strikes.rs`).
- EBDT: `simsun.ttc` carries EBDT strikes; at the strike ppem the glyph is a pure 0/255 bitmap, 7.3 px away it is an antialiased outline.
- sbix: no sbix font is installed or freely available on the test machine; sbix is covered by the synthetic font in `strike.rs` and the shared placement math only.
- Glyph conformance through D3D12 against `aqua-gpu-ref` and DirectWrite (`aqua-oracle-dwrite/tests/d3d12_glyphs.rs`) now runs on WARP and the RX 9070 XT for baseline, bindless, enhanced barriers and both: backend gap to the reference is 1/255 on every combination, ink ratio 0.88 to 1.05.

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
`LaidGlyph { src_byte_start/end, font_id: FontId, glyph_id: u16,
font_size_px, x, y, w, bidi_level }` — glyph `x` has line alignment baked
in; line boxes are the contract for carets/selection, glyph data for
custom canvas work and accessibility.

Geometry (all source coordinates unless noted):

- `layout.caret(byte) -> Option<CaretRect { x, y_top, height }>`
- `layout.hit(x, y) -> Option<usize>` (grapheme-snapped)
- `layout.selection_rects(range) -> Vec<Rect>`
- `measure_text(&mut fonts, text, &style, max_width, scale) -> (w, h)` —
  one-shot helper for `aqua-ui` Taffy measure callbacks (returns unscaled
  logical px). Retained `TextLayout` for repeated measurement.

Caching: per-instance dirty tracking (unchanged inputs skip shaping) plus
`SharedLayoutCache::new(capacity)` LRU sharing `Arc<LaidOut>` across
widgets — `layout_shared(&mut fonts, &mut cache)`. Capacity counts entries
(8–128 typical); hits/misses/evictions counters exposed. Font loads
bump a revision proxy in the cache key, so newly installed fonts reshape.

Bidi note: itemization and reordering are UAX 9 via `unicode-bidi`; line
breaking is UAX 14 via `icu_segmenter`; shaping is HarfRust.
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
- Platform wiring: `aqua-ime-tsf` preedit and commit events feed
  `set_composition/commit_composition`; the candidate window is placed with
  `layout.caret(ed.ime_caret())`. Clipboard/cut/copy use
  `selected_text()`/`delete_selection()`/`insert()`.
## Gaps (not claimed)

- Slug has no GPU consumer yet; scale animation still re-rasterizes into the atlas at quantized sizes.
- Fake bold is not applied to Slug glyphs.
- sbix is unverified against a real font.
- The glyph shader assumes a contrasting backdrop (see Tone).
- No line breaking inside `TextEditor` movement (up/down, page); UI composes `layout` + `hit`.
- No spellcheck or hyphenation dictionaries.
- `FontManager::new_without_system_fonts()` plus `load_font_*` is the hermetic path.
