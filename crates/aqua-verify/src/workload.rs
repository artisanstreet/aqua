use aqua_gpu_ref::conformance::cases::{backdrop_blur, glyph_run, quad, rect, shadow};
use aqua_graph::{Extent, PaintCommand};

pub fn editor_workload(surface: Extent) -> Vec<PaintCommand> {
    let mut commands = vec![quad(
        0.0,
        0.0,
        surface.width as f32,
        surface.height as f32,
        "#1e1e2e",
        1.0,
        0.0,
        0.0,
    )];
    for row in 0..140u32 {
        commands.push(quad(
            280.0,
            40.0 + row as f32 * 9.6,
            2000.0 - (row % 17) as f32 * 40.0,
            6.0,
            "#313244",
            1.0,
            3.0,
            0.0,
        ));
    }
    for panel in 0..6u32 {
        commands.push(shadow(
            rect(40.0 + panel as f32 * 400.0, 900.0, 360.0, 420.0),
            12.0,
            [0.0, 8.0],
            16.0,
            0.0,
            "#000000",
            0.45,
        ));
        commands.push(quad(
            40.0 + panel as f32 * 400.0,
            900.0,
            360.0,
            420.0,
            "#45475a",
            1.0,
            12.0,
            1.0,
        ));
    }
    commands.push(PaintCommand::PushOpacity(0.9));
    commands.push(backdrop_blur(rect(900.0, 300.0, 700.0, 420.0), 24.0, 14.0));
    commands.push(quad(900.0, 300.0, 700.0, 420.0, "#89b4fa", 0.2, 14.0, 1.0));
    commands.push(PaintCommand::PopOpacity);
    let glyphs: Vec<_> = (0..4000u32)
        .map(|index| {
            (
                rect(
                    300.0 + (index % 200) as f32 * 9.0,
                    44.0 + (index / 200) as f32 * 40.0,
                    8.0,
                    14.0,
                ),
                rect(
                    (index % 8) as f32 * 8.0,
                    (index / 8 % 2) as f32 * 16.0,
                    8.0,
                    16.0,
                ),
                "#cdd6f4",
                1.0,
            )
        })
        .collect();
    commands.push(glyph_run(&glyphs));
    commands
}
