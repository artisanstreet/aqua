# Pixel storm: one box per pixel, Aqua vs GPUI

Measured 2026-11-06 on the shared Windows machine (125% scaling, dpi 120), release builds for x86_64-pc-windows-gnu. Other agents were building and running scoreboards at the same time, so treat absolute numbers as noisy; the ratios and the order of magnitude are what matter here.

The Aqua exe was built from the working tree on top of `2de8b44`, which held other agents' uncommitted changes (taffy, layout, runtime and shell) at the time.

## The scene

- Aqua: `crates/aqua/examples/pixel_storm.rs`. GPUI: `tools/pixel-storm-gpui` (gpui-ce rev `6cd20f9c5deab2b160b68ac66f453ec70820e8a2`, the same pin and features as the Editor).
- A column of rows, each row a run of 1x1 boxes (`Aqua::Box` with a background, `div().w(px(1.)).h(px(1.)).bg(..)`). The grid is `--size WxH` logical pixels, so 1280x800 is 1,024,000 boxes. The window client area is the same size in logical pixels.
- Colour per box: red from `x.pow(2)`, green from `y.pow(3)`, blue from `(x*y).pow(2)`, each wrapping mod 256 with a frame term (`3t`, `5t`, `7t`). Red is a running fold over the row (the average of the box's own red and the left neighbour's computed red), and blue is XORed with the left neighbour's red. Both versions use the same pure functions.
- Animated mode (default): every presented frame bumps `t`, so every colour changes every frame. Aqua bumps a signal from `on_frame_completed`; GPUI rerenders from `window.on_next_frame`.
- Static mode (`--static`): colours are computed once. GPUI still rebuilds its element tree every frame (that is how GPUI draws), from cached colours. Aqua asks for a frame after each present, but nothing changed, so its retained scene is never redrawn.
- At 125% scaling GPUI rounds each 1.25 device-pixel box to 1 device pixel, so its grid covers the top-left 80% of the window and the rest is the white window background. Aqua covers the whole client area. The box count is the same in both.

## How it was measured

- `win-app` with `AQUA_NO_RESIZE=1` and `AQUA_STATS_OVERRIDE=off`, steps `cpu c0; gpumem g0; sleep 21; cpu c1; gpumem g1`.
- Frame time: both apps log the interval between consecutive presents (Aqua: `on_frame_completed`; GPUI: `on_next_frame` callbacks) and summarise each 10 s window. The table uses the second window (steady state) where there is one. At 40 frames per window p99 is effectively the max.
- First frame: when PrintWindow first sees the scene (`probe-startup first_content_ms`, from launch). The app's own first-present mark is in brackets. GPUI's own mark fires before the frame is actually on screen, so use the PrintWindow number to compare the two.
- CPU: process CPU time over the 21 s window, as a percentage of one core.
- Private bytes and GPU dedicated memory: at the end of the window (`probe-cpu c1`, `probe-gpumem g1`).

## Results

| Size | Boxes | Version | Mode | First frame ms | Frame p50 ms | p99 ms | max ms | CPU % of a core | Private MB | GPU dedicated MB |
|---|---|---|---|---|---|---|---|---|---|---|
| 1280x800 | 1,024,000 | Aqua | animated | 18,120 (17,420) | ~11,000 | 10,961 | 10,961 | 117 | 7,935 (9,696 at exit) | 541.7 |
| 1280x800 | 1,024,000 | Aqua | static | 16,543 (16,480) | no redraw | no redraw | no redraw | 0.0 | 7,401 | 541.7 |
| 1280x800 | 1,024,000 | GPUI | animated | 17,908 (9,460) | 7,010 | 7,010 | 8,376 | 98.7 | 5,147 | 206.9 |
| 1280x800 | 1,024,000 | GPUI | static | 14,483 (7,542) | 7,074 | 7,276 | 7,276 | 98.4 | 4,954 | 199.9 |
| 640x400 | 256,000 | Aqua | animated | 2,590 (2,381) | 1,260 | 1,345 | 1,345 | 110 | 2,876 | 147.2 |
| 640x400 | 256,000 | Aqua | static | 2,412 (2,355) | no redraw | no redraw | no redraw | 1.0 | 1,910 | 147.2 |
| 640x400 | 256,000 | GPUI | animated | 2,122 (1,098) | 1,190 | 1,244 | 1,244 | 98 | 1,473 | 64.4 |
| 640x400 | 256,000 | GPUI | static | 2,011 (1,048) | 1,125 | 1,198 | 1,198 | 98 | 1,300 | 62.6 |
| 320x200 | 64,000 | Aqua | animated | 523 (487) | 279 | 318 | 318 | 111 | 809 | 48.7 |
| 320x200 | 64,000 | Aqua | static | 522 (456) | no redraw | no redraw | no redraw | 0.3 | 534 | 48.7 |
| 320x200 | 64,000 | GPUI | animated | 766 (411) | 231 | 254 | 254 | 98 | 431 | 28.9 |
| 320x200 | 64,000 | GPUI | static | 548 (307) | 228 | 254 | 254 | 97 | 391 | 28.4 |
| 160x100 | 16,000 | Aqua | animated | 205 (167) | 70.9 | 87.5 | 87.6 | 92 | 269 | 24.2 |
| 160x100 | 16,000 | GPUI | animated | 254 (149) | 48.5 | 66.8 | 77.6 | 95 | 170 | 20.1 |
| 80x50 | 4,000 | Aqua | animated | 170 (111) | 25.0 | 37.6 | 37.7 | 59 | 124 | 18.2 |
| 80x50 | 4,000 | GPUI | animated | 266 (143) | 8.6 | 13.5 | 19.7 | 81 | 106 | 17.4 |

The 1280x800 frame times rest on 2 to 4 intervals per 10 s window. Aqua's animated 1M run grew from 7.9 GB to 9.7 GB private in its last seconds.

## Where each one breaks

- Neither crashed or ran out of memory at 1,024,000 boxes. Both drop to seconds per frame: GPUI about 7 s, Aqua about 11 s.
- 60 fps (16.7 ms): GPUI holds it at 4,000 boxes (p50 8.6 ms) and loses it by 16,000 (48.5 ms). Aqua already misses it at 4,000 (25 ms p50).
- Both scale a little worse than linearly. GPUI goes from 231 ms at 64k to 1.19 s at 256k (5.1x for 4x the boxes) and 7.0 s at 1M (5.9x). Aqua goes from 279 ms to 1.26 s (4.5x) and about 11 s (8.7x).
- Memory: at 1M boxes Aqua uses 1.5 to 1.9x GPUI's private bytes and 2.6x its GPU dedicated memory. That is about 7.7 KB of private memory per box for Aqua and 5 KB for GPUI.
- Static: Aqua's retained scene costs nothing once drawn (0 to 1% CPU, working set trimmed to a few MB). GPUI pays its full frame cost every frame whether or not anything changed, because it rebuilds and relays out the whole tree.

## What dominates Aqua's frame

Profiled at 640x400 animated (`AQUA_STATS_OVERRIDE=profile AQUA_PROFILE=1`, 256,000 boxes, about 1.3 s per frame):

| Thread | Phase | ms per frame |
|---|---|---|
| app | build (reconcile and patch the retained tree: 1.54M nodes visited, 256k scene items patched) | 1,160 to 1,305 |
| app | reactive (the render closure building 256k `Element`s with the colour math) | 137 to 148 |
| app | layout (no relayout, colours only) | 6 to 8 |
| render | lower | 58 to 65 |
| render | plan | 35 to 39 |
| render | record | 29 to 31 |
| render | present | under 1 |

About 85% of the frame is the app thread's `build` phase, which diffs the new element tree against the retained one and patches every scene item. Building the elements, layout and the whole GPU side together are under 20%. Because the next frame starts only after the previous present, the app-thread and render-thread costs add up rather than overlap.
