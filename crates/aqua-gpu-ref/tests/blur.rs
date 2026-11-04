use aqua_gpu_ref::RefBackend;
use aqua_gpu_ref::conformance::Session;
use aqua_gpu_ref::conformance::cases::{backdrop_blur, build, hex, quad, rect};
use aqua_graph::{Extent, PaintCommand};

fn blurred_row(radius: f32, scene: &[PaintCommand], extent: Extent) -> Vec<f32> {
    let mut commands = scene.to_vec();
    commands.push(backdrop_blur(
        rect(0.0, 0.0, extent.width as f32, extent.height as f32),
        radius,
        0.0,
    ));
    let case = build("blur", extent, hex("#000000", 1.0), &commands);
    let mut backend = RefBackend::new();
    let pixels = Session::new(&mut backend)
        .render_case(&case)
        .expect("renders");
    let y = extent.height as usize / 2;
    (0..extent.width as usize)
        .map(|x| pixels.rgba8[(y * extent.width as usize + x) * 4] as f32 / 255.0)
        .collect()
}

fn impulse(extent: Extent) -> Vec<PaintCommand> {
    vec![quad(
        extent.width as f32 / 2.0 - 1.0,
        0.0,
        2.0,
        extent.height as f32,
        "#ffffff",
        1.0,
        0.0,
        0.0,
    )]
}

fn spread(row: &[f32]) -> f32 {
    let total: f32 = row.iter().sum();
    let mean: f32 = row
        .iter()
        .enumerate()
        .map(|(x, v)| (x as f32 + 0.5) * v)
        .sum::<f32>()
        / total;
    (row.iter()
        .enumerate()
        .map(|(x, v)| (x as f32 + 0.5 - mean).powi(2) * v)
        .sum::<f32>()
        / total)
        .sqrt()
}

#[test]
fn blur_of_a_uniform_backdrop_is_uniform() {
    let extent = Extent::new(64, 32);
    let scene = [quad(0.0, 0.0, 64.0, 32.0, "#ffffff", 0.5, 0.0, 0.0)];
    for radius in [0.5, 3.0, 9.0, 17.0] {
        let row = blurred_row(radius, &scene, extent);
        let (low, high) = row
            .iter()
            .fold((1.0f32, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
        assert!(high - low <= 2.0 / 255.0, "radius {radius}: {low}..{high}");
    }
}

#[test]
fn blurred_impulse_is_symmetric_and_conserves_most_of_its_mass() {
    let extent = Extent::new(128, 16);
    let source = blurred_row(0.01, &impulse(extent), extent);
    let row = blurred_row(8.0, &impulse(extent), extent);
    let source_mass: f32 = source.iter().sum();
    let mass: f32 = row.iter().sum();
    assert!(
        (mass / source_mass - 1.0).abs() < 0.04,
        "{mass} vs {source_mass}"
    );
    for offset in 1..40 {
        let (left, right) = (row[64 - 1 - offset], row[64 + offset]);
        assert!(
            (left - right).abs() < 3.0 / 255.0,
            "offset {offset}: {left} vs {right}"
        );
    }
    assert!(row[63] > row[40] && row[40] >= row[20]);
}

#[test]
#[ignore = "dual Kawase level changes step the width at every power-of-two radius; planner must keep the blur continuous (ARCHITECTURE section 6.2)"]
fn blur_width_grows_smoothly_and_monotonically_with_the_radius() {
    let extent = Extent::new(160, 16);
    let widths: Vec<f32> = (1..=40)
        .map(|step| spread(&blurred_row(step as f32 * 0.4, &impulse(extent), extent)))
        .collect();
    for pair in widths.windows(2) {
        assert!(
            pair[1] >= pair[0] - 0.05,
            "width regressed: {} -> {}",
            pair[0],
            pair[1]
        );
        assert!(
            pair[1] - pair[0] < 0.9,
            "width jumped: {} -> {}",
            pair[0],
            pair[1]
        );
    }
    assert!(widths[39] > widths[4] * 2.0);
}

#[test]
#[ignore = "dual Kawase level changes step the output at radius 2, 4, 8 and 16 (ARCHITECTURE section 6.2)"]
fn level_count_changes_do_not_step_the_output() {
    let extent = Extent::new(128, 16);
    for boundary in [2.0f32, 4.0, 8.0, 16.0] {
        let below = blurred_row(boundary - 0.02, &impulse(extent), extent);
        let above = blurred_row(boundary + 0.02, &impulse(extent), extent);
        let worst = below
            .iter()
            .zip(&above)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 12.0 / 255.0,
            "radius {boundary}: levels change moved a pixel by {worst}"
        );
    }
}

#[test]
fn backdrop_blur_only_touches_its_bounds() {
    let extent = Extent::new(64, 32);
    let mut commands = vec![
        quad(0.0, 0.0, 64.0, 32.0, "#202830", 1.0, 0.0, 0.0),
        quad(30.0, 0.0, 4.0, 32.0, "#ffffff", 1.0, 0.0, 0.0),
    ];
    commands.push(backdrop_blur(rect(20.0, 4.0, 24.0, 24.0), 6.0, 0.0));
    let case = build("bounds", extent, hex("#000000", 0.0), &commands);
    let mut backend = RefBackend::new();
    let pixels = Session::new(&mut backend)
        .render_case(&case)
        .expect("renders");
    let unblurred = {
        let case = build("plain", extent, hex("#000000", 0.0), &commands[..2]);
        Session::new(&mut backend)
            .render_case(&case)
            .expect("renders")
    };
    for y in 0..32usize {
        for x in 0..64usize {
            let inside = (20..44).contains(&x) && (4..28).contains(&y);
            let same =
                pixels.rgba8[(y * 64 + x) * 4..][..4] == unblurred.rgba8[(y * 64 + x) * 4..][..4];
            if !inside {
                assert!(same, "({x},{y}) changed outside the blur bounds");
            }
        }
    }
    assert!(pixels.rgba8[(16 * 64 + 31) * 4] < unblurred.rgba8[(16 * 64 + 31) * 4]);
}
