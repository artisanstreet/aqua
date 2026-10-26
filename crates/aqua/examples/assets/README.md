# `examples/assets` fixtures (media3d owner)

Decoding fixtures for `aqua-render/src/media.rs` tests (embedded via
`include_bytes!`), plus the documented glTF import fixture. Regenerate with:

```sh
python3 crates/aqua/examples/assets/gen_gltf.py   # sample_cube.gltf
# PNG/BMP fixtures: see the generator note below; bytes verified by tests.
```

| File | Contents | Exercises |
|---|---|---|
| `sample_rgba.png` | 4x3 RGBA: opaque primaries, half-red, transparent, 78%-gray | exact pixels, straight→premult, alpha paths |
| `sample_gray_trns.png` | 2x2 gray8 + `tRNS` (0 → transparent) | gray + transparency extension |
| `sample_palette.png` | 2x2 palette + `tRNS` (index 3 → a=64) | palette + per-index alpha |
| `sample_16bit.png` | 2x2 RGB 16-bit primaries/white | depth downscale |
| `sample_fixed.png` | same pixels, fixed-Huffman DEFLATE | codec block-type coverage |
| `sample_interlaced.png` | proper Adam7 (7 passes, filter bytes per sub-row) | interlace handling (mature codec) |
| `sample_rgb24.bmp` | 3x2 24-bit bottom-up (blue/green/red, white/black/gray) | BMP row order, BGR→RGBA |
| `sample_rgba32.bmp` | 2x2 32-bit with alpha (incl. a=0 and a=200) | BMP alpha, transparent texels |
| `sample_cube.gltf` | glTF 2.0 cube, embedded buffer (8 pos + 36 idx) | import-path fixture: decodes to `MeshData::cube()` |

`sample_cube.gltf` is NOT parsed by any shipped code yet (no fake loader):
it pins the §5 mapping in `docs/GPU_EXTENSIONS.md` so the future `gltf`
loader has a known-good fixture. `gen_gltf.py` is kept so the JSON can be
regenerated rather than hand-edited.
