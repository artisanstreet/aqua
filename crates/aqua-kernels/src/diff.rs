pub fn reference(a: &[u8], b: &[u8]) -> Option<u8> {
    (a.len() == b.len()).then(|| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0))
}

#[cfg(target_arch = "x86_64")]
mod sse2 {
    use core::arch::x86_64::*;

    pub fn max_abs_diff(a: &[u8], b: &[u8]) -> u8 {
        let blocks = a.len() / 16;
        let mut best = 0u8;
        unsafe {
            let mut accumulator = _mm_setzero_si128();
            for block in 0..blocks {
                let x = _mm_loadu_si128(a.as_ptr().add(block * 16) as *const __m128i);
                let y = _mm_loadu_si128(b.as_ptr().add(block * 16) as *const __m128i);
                let difference = _mm_or_si128(_mm_subs_epu8(x, y), _mm_subs_epu8(y, x));
                accumulator = _mm_max_epu8(accumulator, difference);
            }
            let mut lanes = [0u8; 16];
            _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, accumulator);
            best = lanes.iter().copied().fold(best, u8::max);
        }
        a[blocks * 16..]
            .iter()
            .zip(&b[blocks * 16..])
            .map(|(x, y)| x.abs_diff(*y))
            .fold(best, u8::max)
    }
}

pub fn max_abs_diff(a: &[u8], b: &[u8]) -> Option<u8> {
    if a.len() != b.len() {
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    {
        Some(sse2::max_abs_diff(a, b))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        reference(a, b)
    }
}
