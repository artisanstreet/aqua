use crate::Pixel;

pub fn reference_pixel(destination: Pixel, source: Pixel, opacity: f32) -> Pixel {
    let scaled = source.map(|channel| channel * opacity);
    let keep = 1.0 - scaled[3];
    std::array::from_fn(|channel| scaled[channel] + destination[channel] * keep)
}

pub fn reference(destination: &mut [Pixel], source: &[Pixel], opacity: f32) {
    destination
        .iter_mut()
        .zip(source)
        .for_each(|(pixel, source)| *pixel = reference_pixel(*pixel, *source, opacity));
}

#[cfg(target_arch = "x86_64")]
mod sse2 {
    use core::arch::x86_64::*;

    use crate::Pixel;

    pub fn apply(destination: &mut [Pixel], source: &[Pixel], opacity: f32) {
        let count = destination.len().min(source.len());
        let target = destination.as_mut_ptr() as *mut f32;
        let colors = source.as_ptr() as *const f32;
        unsafe {
            let factor = _mm_set1_ps(opacity);
            let one = _mm_set1_ps(1.0);
            for index in 0..count {
                let scaled = _mm_mul_ps(_mm_loadu_ps(colors.add(index * 4)), factor);
                let keep = _mm_sub_ps(one, _mm_shuffle_ps::<0xFF>(scaled, scaled));
                let slot = target.add(index * 4);
                let result = _mm_add_ps(scaled, _mm_mul_ps(_mm_loadu_ps(slot), keep));
                _mm_storeu_ps(slot, result);
            }
        }
    }
}

pub fn apply(destination: &mut [Pixel], source: &[Pixel], opacity: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        sse2::apply(destination, source, opacity)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        reference(destination, source, opacity)
    }
}
