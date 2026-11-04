use crate::Pixel;

pub const IDENTITY: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    0.0,
];

#[allow(clippy::manual_clamp)]
fn clamp_unit(value: f32) -> f32 {
    value.max(0.0).min(1.0)
}

pub fn reference_pixel(matrix: &[f32; 20], pixel: Pixel) -> Pixel {
    let [r, g, b, a] = pixel;
    let (sr, sg, sb) = match a > 0.0 {
        true => (r / a, g / a, b / a),
        false => (0.0, 0.0, 0.0),
    };
    let row = |index: usize| {
        let m = &matrix[index * 5..index * 5 + 5];
        clamp_unit(m[0] * sr + m[1] * sg + m[2] * sb + m[3] * a + m[4])
    };
    let out = [row(0), row(1), row(2), row(3)];
    [out[0] * out[3], out[1] * out[3], out[2] * out[3], out[3]]
}

pub fn reference(matrix: &[f32; 20], pixels: &mut [Pixel]) {
    pixels
        .iter_mut()
        .for_each(|pixel| *pixel = reference_pixel(matrix, *pixel));
}

#[cfg(target_arch = "x86_64")]
mod sse2 {
    use core::arch::x86_64::*;

    use crate::Pixel;

    pub fn apply(matrix: &[f32; 20], pixels: &mut [Pixel]) {
        let column = |j: usize| [matrix[j], matrix[5 + j], matrix[10 + j], matrix[15 + j]];
        let (c0, c1, c2, c3, c4) = (column(0), column(1), column(2), column(3), column(4));
        let colors = pixels.as_mut_ptr() as *mut f32;
        let count = pixels.len();
        unsafe {
            let m0 = _mm_loadu_ps(c0.as_ptr());
            let m1 = _mm_loadu_ps(c1.as_ptr());
            let m2 = _mm_loadu_ps(c2.as_ptr());
            let m3 = _mm_loadu_ps(c3.as_ptr());
            let m4 = _mm_loadu_ps(c4.as_ptr());
            let zero = _mm_setzero_ps();
            let one = _mm_set1_ps(1.0);
            let rgb = _mm_castsi128_ps(_mm_set_epi32(0, -1, -1, -1));
            for index in 0..count {
                let slot = colors.add(index * 4);
                let pixel = _mm_loadu_ps(slot);
                let alpha = _mm_shuffle_ps::<0xFF>(pixel, pixel);
                let visible = _mm_cmpgt_ps(alpha, zero);
                let straight = _mm_and_ps(_mm_div_ps(pixel, alpha), visible);
                let r = _mm_shuffle_ps::<0x00>(straight, straight);
                let g = _mm_shuffle_ps::<0x55>(straight, straight);
                let b = _mm_shuffle_ps::<0xAA>(straight, straight);
                let mut sum = _mm_mul_ps(m0, r);
                sum = _mm_add_ps(sum, _mm_mul_ps(m1, g));
                sum = _mm_add_ps(sum, _mm_mul_ps(m2, b));
                sum = _mm_add_ps(sum, _mm_mul_ps(m3, alpha));
                sum = _mm_add_ps(sum, m4);
                sum = _mm_min_ps(_mm_max_ps(sum, zero), one);
                let out_alpha = _mm_shuffle_ps::<0xFF>(sum, sum);
                let premultiplied = _mm_mul_ps(sum, out_alpha);
                let result = _mm_or_ps(_mm_and_ps(premultiplied, rgb), _mm_andnot_ps(rgb, sum));
                _mm_storeu_ps(slot, result);
            }
        }
    }
}

pub fn apply(matrix: &[f32; 20], pixels: &mut [Pixel]) {
    #[cfg(target_arch = "x86_64")]
    {
        sse2::apply(matrix, pixels)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        reference(matrix, pixels)
    }
}
