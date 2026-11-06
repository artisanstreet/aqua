use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_graph::{Extent, PaintCommand};

use super::cases::{backdrop_blur, build, clip, glyph_run, hex, photo, quad, rect, shadow, Case};

const WIDE: Extent = Extent::new(160, 112);
const CELL: f32 = 16.0;

fn text_rows(extent: Extent, ink: &str, alpha: f32) -> PaintCommand {
    let columns = (extent.width as f32 / (CELL + 2.0)) as usize;
    let rows = (extent.height as f32 / (CELL + 2.0)) as usize;
    let sources = [rect(0.0, 0.0, 16.0, 16.0), rect(32.0, 0.0, 16.0, 16.0)];
    let glyphs: Vec<(Rect, Rect, &str, f32)> = (0..rows)
        .flat_map(|row| {
            (0..columns).map(move |column| {
                (
                    rect(
                        1.0 + column as f32 * (CELL + 2.0),
                        1.0 + row as f32 * (CELL + 2.0),
                        CELL,
                        CELL,
                    ),
                    sources[(row + column) % 2],
                    ink,
                    alpha,
                )
            })
        })
        .collect();
    glyph_run(&glyphs)
}

fn page(extent: Extent, paper: &str, ink: &str) -> Vec<PaintCommand> {
    vec![
        quad(
            0.0,
            0.0,
            extent.width as f32,
            extent.height as f32,
            paper,
            1.0,
            0.0,
            0.0,
        ),
        text_rows(extent, ink, 1.0),
    ]
}

fn rounded(bounds: Rect, radius: f32) -> PaintCommand {
    PaintCommand::PushRoundedClip {
        rect: bounds,
        radius,
    }
}

fn corner_piece(
    bounds: Rect,
    radius: f32,
    quadrant: (f32, f32),
    content: &[PaintCommand],
) -> Vec<PaintCommand> {
    let half_w = (bounds.w / 2.0).floor();
    let half_h = (bounds.h / 2.0).floor();
    let piece = rect(
        bounds.x + quadrant.0 * half_w,
        bounds.y + quadrant.1 * half_h,
        if quadrant.0 == 0.0 {
            half_w
        } else {
            bounds.w - half_w
        },
        if quadrant.1 == 0.0 {
            half_h
        } else {
            bounds.h - half_h
        },
    );
    let mut out = vec![
        clip(piece.x, piece.y, piece.w, piece.h),
        rounded(bounds, radius),
    ];
    out.extend_from_slice(content);
    out.extend([PaintCommand::PopClip, PaintCommand::PopClip]);
    out
}

fn per_corner(bounds: Rect, radii: [f32; 4], content: &[PaintCommand]) -> Vec<PaintCommand> {
    [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .zip(radii)
        .flat_map(|(quadrant, radius)| corner_piece(bounds, radius, quadrant, content))
        .collect()
}

fn top_rounded(bounds: Rect, radius: f32, content: &[PaintCommand]) -> Vec<PaintCommand> {
    let mut out = vec![
        clip(bounds.x, bounds.y, bounds.w, bounds.h),
        rounded(
            rect(bounds.x, bounds.y, bounds.w, bounds.h + radius * 2.0),
            radius,
        ),
    ];
    out.extend_from_slice(content);
    out.extend([PaintCommand::PopClip, PaintCommand::PopClip]);
    out
}

fn glass_pane(bounds: Rect, blur: f32, radius: f32) -> Vec<PaintCommand> {
    vec![
        backdrop_blur(bounds, blur, radius),
        quad(
            bounds.x, bounds.y, bounds.w, bounds.h, "#202020", 0.3, radius, 0.0,
        ),
        quad(
            bounds.x + 6.0,
            bounds.y + 6.0,
            bounds.w - 12.0,
            bounds.h - 12.0,
            "#ffffff",
            0.04,
            (radius - 6.0).max(0.0),
            0.0,
        ),
    ]
}

fn blur_over_text() -> Vec<Case> {
    let pane = rect(24.0, 24.0, 112.0, 64.0);
    let mut light_on_dark = page(WIDE, "#000000", "#ffffff");
    light_on_dark.push(backdrop_blur(pane, 8.0, 14.0));
    let mut dark_on_light = page(WIDE, "#ffffff", "#000000");
    dark_on_light.push(backdrop_blur(pane, 8.0, 14.0));
    let mut wide_radius = page(WIDE, "#000000", "#ffffff");
    wide_radius.push(backdrop_blur(rect(8.0, 8.0, 144.0, 96.0), 24.0, 20.0));
    let mut inside_group = page(WIDE, "#000000", "#ffffff");
    inside_group.extend([
        PaintCommand::PushOpacity(0.7),
        backdrop_blur(pane, 6.0, 10.0),
        PaintCommand::PopOpacity,
    ]);
    let mut under_clip = page(WIDE, "#000000", "#ffffff");
    under_clip.extend([
        clip(40.0, 30.0, 80.0, 50.0),
        backdrop_blur(rect(8.0, 8.0, 144.0, 96.0), 10.0, 0.0),
        PaintCommand::PopClip,
    ]);
    [
        ("blur_over_text_light_on_dark", light_on_dark),
        ("blur_over_text_dark_on_light", dark_on_light),
        ("blur_over_text_wide_radius", wide_radius),
        ("blur_over_text_in_group", inside_group),
        ("blur_over_text_clipped", under_clip),
    ]
    .into_iter()
    .map(|(name, commands)| build(name, WIDE, Oklch::TRANSPARENT, &commands))
    .collect()
}

fn glass_over_text() -> Vec<Case> {
    let pane = rect(20.0, 28.0, 120.0, 56.0);
    let mut plain = page(WIDE, "#101010", "#f0f0f0");
    plain.extend(glass_pane(pane, 12.0, 16.0));
    let mut with_label = page(WIDE, "#101010", "#f0f0f0");
    with_label.extend(glass_pane(pane, 12.0, 16.0));
    with_label.push(glyph_run(&[
        (
            rect(32.0, 40.0, 16.0, 16.0),
            rect(0.0, 0.0, 16.0, 16.0),
            "#ffffff",
            1.0,
        ),
        (
            rect(52.0, 40.0, 16.0, 16.0),
            rect(32.0, 0.0, 16.0, 16.0),
            "#ffffff",
            0.8,
        ),
        (
            rect(72.0, 40.0, 16.0, 16.0),
            rect(16.0, 0.0, 16.0, 16.0),
            "#ece030",
            1.0,
        ),
    ]));
    let mut light = page(WIDE, "#f4f4f4", "#101010");
    light.extend(glass_pane(pane, 12.0, 16.0));
    let mut stacked = page(WIDE, "#101010", "#f0f0f0");
    stacked.extend(glass_pane(rect(12.0, 12.0, 136.0, 88.0), 10.0, 18.0));
    stacked.push(text_rows(Extent::new(120, 72), "#ffffff", 0.6));
    stacked.extend(glass_pane(rect(40.0, 40.0, 80.0, 40.0), 6.0, 12.0));
    [
        ("glass_over_text_plain", plain),
        ("glass_over_text_with_label", with_label),
        ("glass_over_text_light_page", light),
        ("glass_over_text_stacked", stacked),
    ]
    .into_iter()
    .map(|(name, commands)| build(name, WIDE, Oklch::TRANSPARENT, &commands))
    .collect()
}

fn shadow_cases() -> Vec<Case> {
    let card = rect(30.0, 24.0, 100.0, 56.0);
    let mut elevated = page(WIDE, "#d8dce4", "#a0a8b8");
    elevated.extend([
        shadow(card, 14.0, [0.0, 2.0], 3.0, 0.0, "#000000", 0.2),
        shadow(card, 14.0, [0.0, 14.0], 18.0, -4.0, "#000000", 0.35),
        quad(card.x, card.y, card.w, card.h, "#fbfbfd", 1.0, 14.0, 0.0),
    ]);
    let mut dark = page(WIDE, "#0c0c10", "#303844");
    dark.extend([
        shadow(card, 14.0, [0.0, 10.0], 12.0, 0.0, "#000000", 0.8),
        quad(card.x, card.y, card.w, card.h, "#23262e", 1.0, 14.0, 1.0),
    ]);
    let mut under_glass = page(WIDE, "#14161c", "#e8e8f0");
    under_glass.extend([
        shadow(card, 18.0, [0.0, 12.0], 14.0, 0.0, "#000000", 0.6),
        PaintCommand::Shadow(aqua_graph::Shadow {
            bounds: card,
            radius: 18.0,
            offset: [0.0, -1.0],
            sigma: 0.0,
            spread: 0.0,
            color: hex("#ffffff", 0.14),
            cast: aqua_graph::ShadowCast::Outside,
        }),
    ]);
    under_glass.extend(glass_pane(card, 12.0, 18.0));
    let mut clipped = page(WIDE, "#d8dce4", "#a0a8b8");
    clipped.extend([
        rounded(rect(10.0, 10.0, 140.0, 92.0), 20.0),
        shadow(
            rect(-20.0, 40.0, 120.0, 80.0),
            16.0,
            [0.0, 6.0],
            10.0,
            4.0,
            "#102040",
            0.6,
        ),
        PaintCommand::PopClip,
    ]);
    let mut grouped = page(WIDE, "#d8dce4", "#a0a8b8");
    grouped.extend([
        PaintCommand::PushOpacity(0.55),
        shadow(card, 14.0, [4.0, 10.0], 8.0, 2.0, "#000000", 0.7),
        quad(card.x, card.y, card.w, card.h, "#fbfbfd", 1.0, 14.0, 0.0),
        PaintCommand::PopOpacity,
    ]);
    [
        ("shadow_two_layer_elevation", elevated),
        ("shadow_heavy_on_dark", dark),
        ("shadow_under_glass_with_rim", under_glass),
        ("shadow_clipped_by_rounded_clip", clipped),
        ("shadow_in_translucent_group", grouped),
    ]
    .into_iter()
    .map(|(name, commands)| build(name, WIDE, Oklch::TRANSPARENT, &commands))
    .collect()
}

fn emoji_cases() -> Vec<Case> {
    let full = rect(0.0, 0.0, 32.0, 32.0);
    let mut line = page(WIDE, "#14161c", "#e8e8f0");
    line.extend([
        photo(rect(8.0, 8.0, 32.0, 32.0), full, 0.0),
        photo(rect(48.0, 8.0, 20.0, 20.0), full, 0.0),
        photo(rect(76.0, 8.0, 14.0, 14.0), full, 0.0),
        photo(rect(98.0, 8.5, 20.0, 20.0), full, 0.0),
        photo(rect(8.25, 52.75, 32.0, 32.0), full, 0.0),
        photo(rect(48.0, 52.0, 48.0, 48.0), full, 0.0),
    ]);
    let mut over_text = page(WIDE, "#000000", "#ffffff");
    over_text.extend([
        photo(rect(20.0, 20.0, 24.0, 24.0), full, 0.0),
        photo(rect(60.0, 30.0, 24.0, 24.0), full, 0.0),
        PaintCommand::PushOpacity(0.5),
        photo(rect(100.0, 40.0, 32.0, 32.0), full, 0.0),
        PaintCommand::PopOpacity,
    ]);
    let mut glass = page(WIDE, "#000000", "#ffffff");
    glass.extend(glass_pane(rect(20.0, 28.0, 120.0, 56.0), 12.0, 16.0));
    glass.extend([
        photo(rect(30.0, 40.0, 28.0, 28.0), full, 0.0),
        glyph_run(&[
            (
                rect(66.0, 44.0, 16.0, 16.0),
                rect(0.0, 0.0, 16.0, 16.0),
                "#ffffff",
                1.0,
            ),
            (
                rect(86.0, 44.0, 16.0, 16.0),
                rect(32.0, 0.0, 16.0, 16.0),
                "#ffffff",
                1.0,
            ),
        ]),
        photo(
            rect(110.0, 40.0, 24.0, 24.0),
            rect(8.0, 8.0, 16.0, 16.0),
            0.0,
        ),
    ]);
    [
        ("emoji_sizes_and_fractional_origins", line),
        ("emoji_over_high_contrast_text", over_text),
        ("emoji_and_text_on_glass", glass),
    ]
    .into_iter()
    .map(|(name, commands)| build(name, WIDE, Oklch::TRANSPARENT, &commands))
    .collect()
}

fn corner_cases() -> Vec<Case> {
    let bounds = rect(16.0, 14.0, 128.0, 84.0);
    let fill = [
        quad(
            bounds.x, bounds.y, bounds.w, bounds.h, "#4060e0", 0.9, 0.0, 0.0,
        ),
        quad(
            bounds.x + 24.0,
            bounds.y + 12.0,
            70.0,
            40.0,
            "#ece030",
            0.7,
            0.0,
            0.0,
        ),
        photo(
            rect(bounds.x + 70.0, bounds.y + 36.0, 48.0, 40.0),
            rect(0.0, 0.0, 32.0, 32.0),
            0.0,
        ),
    ];
    let mut distinct = page(WIDE, "#101418", "#384050");
    distinct.extend(per_corner(bounds, [6.0, 28.0, 18.0, 2.0], &fill));
    let mut chip = page(WIDE, "#101418", "#384050");
    chip.extend(top_rounded(
        rect(24.0, 40.0, 112.0, 40.0),
        14.0,
        &[quad(24.0, 40.0, 112.0, 40.0, "#e8e8f0", 1.0, 0.0, 0.0)],
    ));
    let mut zero = page(WIDE, "#101418", "#384050");
    zero.extend(per_corner(bounds, [0.0, 0.0, 24.0, 24.0], &fill));
    let mut glass = page(WIDE, "#000000", "#ffffff");
    glass.extend(per_corner(
        rect(20.0, 24.0, 120.0, 64.0),
        [4.0, 24.0, 4.0, 24.0],
        &glass_pane(rect(20.0, 24.0, 120.0, 64.0), 10.0, 0.0),
    ));
    let mut grouped = page(WIDE, "#101418", "#384050");
    grouped.push(PaintCommand::PushOpacity(0.6));
    grouped.extend(per_corner(bounds, [20.0, 8.0, 20.0, 8.0], &fill));
    grouped.push(PaintCommand::PopOpacity);
    [
        ("corner_clip_four_distinct_radii", distinct),
        ("corner_clip_top_rounded_chip", chip),
        ("corner_clip_bottom_only_rounded", zero),
        ("corner_clip_glass_alternating", glass),
        ("corner_clip_in_translucent_group", grouped),
    ]
    .into_iter()
    .map(|(name, commands)| build(name, WIDE, Oklch::TRANSPARENT, &commands))
    .collect()
}

pub fn cases() -> Vec<Case> {
    let mut cases = blur_over_text();
    cases.extend(glass_over_text());
    cases.extend(shadow_cases());
    cases.extend(emoji_cases());
    cases.extend(corner_cases());
    cases
}
