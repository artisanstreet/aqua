use aqua_kernels::{Pixel, color_matrix, diff, over};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    fn pixel(&mut self) -> Pixel {
        let alpha = match self.next() {
            value if value < 0.15 => 0.0,
            value => value,
        };
        [self.next() * alpha, self.next() * alpha, self.next() * alpha, alpha]
    }
}

fn random_matrix(rng: &mut Lcg) -> [f32; 20] {
    std::array::from_fn(|_| rng.next() * 3.0 - 1.0)
}

#[test]
fn color_matrix_fast_path_is_bit_identical_to_the_twin() {
    let mut rng = Lcg(7);
    for length in [0, 1, 2, 3, 17, 255] {
        let matrix = random_matrix(&mut rng);
        let source: Vec<Pixel> = (0..length).map(|_| rng.pixel()).collect();
        let (mut fast, mut twin) = (source.clone(), source);
        color_matrix::apply(&matrix, &mut fast);
        color_matrix::reference(&matrix, &mut twin);
        assert_eq!(fast, twin);
    }
}

#[test]
fn identity_matrix_preserves_premultiplied_pixels() {
    let mut rng = Lcg(11);
    let source: Vec<Pixel> = (0..64).map(|_| rng.pixel()).collect();
    let mut result = source.clone();
    color_matrix::reference(&color_matrix::IDENTITY, &mut result);
    source.iter().zip(&result).for_each(|(want, got)| {
        want.iter()
            .zip(got)
            .for_each(|(w, g)| assert!((w - g).abs() < 1.0e-5, "{want:?} vs {got:?}"))
    });
}

#[test]
fn matrix_offsets_apply_to_straight_color_and_repremultiply() {
    let matrix = [
        0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        1.0, 0.0,
    ];
    let result = color_matrix::reference_pixel(&matrix, [0.0, 0.0, 0.0, 0.5]);
    assert_eq!(result, [0.5, 0.0, 0.0, 0.5]);
}

#[test]
fn over_fast_path_is_bit_identical_to_the_twin() {
    let mut rng = Lcg(23);
    for (length, opacity) in [(0, 1.0), (1, 0.5), (5, 0.37), (130, 1.0)] {
        let destination: Vec<Pixel> = (0..length).map(|_| rng.pixel()).collect();
        let source: Vec<Pixel> = (0..length).map(|_| rng.pixel()).collect();
        let (mut fast, mut twin) = (destination.clone(), destination);
        over::apply(&mut fast, &source, opacity);
        over::reference(&mut twin, &source, opacity);
        assert_eq!(fast, twin);
    }
}

#[test]
fn diff_fast_path_matches_the_twin_and_rejects_length_mismatch() {
    let mut rng = Lcg(31);
    for length in [0usize, 1, 15, 16, 17, 1000] {
        let a: Vec<u8> = (0..length).map(|_| (rng.next() * 256.0) as u8).collect();
        let b: Vec<u8> = (0..length).map(|_| (rng.next() * 256.0) as u8).collect();
        assert_eq!(diff::max_abs_diff(&a, &b), diff::reference(&a, &b));
    }
    assert_eq!(diff::max_abs_diff(&[1, 2], &[1]), None);
}
