#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowRange {
    pub start: usize,
    pub end: usize,
}

impl RowRange {
    #[must_use]
    pub fn empty(at: usize) -> Self {
        Self { start: at, end: at }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    #[must_use]
    pub fn contains(&self, index: usize) -> bool {
        index >= self.start && index < self.end
    }
}

#[derive(Debug, Clone)]
pub struct PrefixSums {
    n: usize,
    bit: Vec<f64>,
}

impl PrefixSums {
    #[must_use]
    pub fn build(heights: &[f32]) -> Self {
        let n = heights.len();
        let mut bit = vec![0.0f64; n + 1];
        for (i, h) in heights.iter().enumerate() {
            bit[i + 1] = *h as f64;
        }
        for i in 1..=n {
            let j = i + (i & i.wrapping_neg());
            if j <= n {
                bit[j] += bit[i];
            }
        }
        Self { n, bit }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.n
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn add(&mut self, index: usize, delta: f64) {
        if index >= self.n || delta == 0.0 {
            return;
        }
        let mut i = index + 1;
        while i <= self.n {
            self.bit[i] += delta;
            i += i & i.wrapping_neg();
        }
    }

    #[must_use]
    pub fn prefix(&self, end: usize) -> f64 {
        let mut i = end.min(self.n);
        let mut out = 0.0;
        while i > 0 {
            out += self.bit[i];
            i -= i & i.wrapping_neg();
        }
        out
    }

    #[must_use]
    pub fn total(&self) -> f64 {
        self.prefix(self.n)
    }

    #[must_use]
    pub fn index_at(&self, offset: f64) -> usize {
        if self.n == 0 || offset < 0.0 {
            return 0;
        }
        let mut idx = 0usize;
        let mut step = 1usize;
        while step <= self.n {
            step <<= 1;
        }
        let mut acc = 0.0;
        while step > 0 {
            let next = idx + step;
            if next <= self.n && acc + self.bit[next] <= offset {
                acc += self.bit[next];
                idx = next;
            }
            step >>= 1;
        }
        idx.min(self.n)
    }
}

#[must_use]
pub fn window_for_viewport(
    sums: &PrefixSums,
    len: usize,
    scroll_y: f32,
    viewport_h: f32,
    overscan: usize,
) -> RowRange {
    if len == 0 {
        return RowRange::empty(0);
    }
    if !viewport_h.is_finite() || viewport_h <= 0.0 {
        let end = (overscan.saturating_mul(2).saturating_add(1).min(len).max(1)).min(len);
        return RowRange { start: 0, end };
    }
    let total = sums.total() as f32;
    let max_scroll = (total - viewport_h).max(0.0);
    let y = if !scroll_y.is_finite() {
        if scroll_y > 0.0 {
            max_scroll
        } else {
            0.0
        }
    } else {
        scroll_y.clamp(0.0, max_scroll)
    };
    let first = sums.index_at(y as f64).min(len.saturating_sub(1));
    let bottom = (y as f64 + viewport_h as f64).min(f64::MAX);
    let last_inclusive = sums.index_at(bottom).min(len.saturating_sub(1));
    let start = first.saturating_sub(overscan);
    let end = last_inclusive
        .saturating_add(1)
        .saturating_add(overscan)
        .min(len);
    RowRange {
        start,
        end: end.max(start.saturating_add(1).min(len)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingAnchor {
    pub key: u64,
    pub offset_in_row: f32,
}

impl ReadingAnchor {
    #[must_use]
    pub fn restore_scroll(&self, sums: &PrefixSums, index: usize, viewport_h: f32) -> f32 {
        let total = sums.total() as f32;
        let target = sums.prefix(index) as f32 + self.offset_in_row;
        target.clamp(0.0, (total - viewport_h).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenwick_matches_naive_prefix() {
        let h: Vec<f32> = (0..500).map(|i| 20.0 + (i % 7) as f32).collect();
        let sums = PrefixSums::build(&h);
        let mut acc = 0.0;
        for (i, v) in h.iter().enumerate() {
            assert!((sums.prefix(i) - acc).abs() < 1e-6);
            acc += *v as f64;
        }
        assert!((sums.total() - acc).abs() < 1e-3);
    }

    #[test]
    fn index_at_matches_linear_scan() {
        let h: Vec<f32> = (0..300).map(|i| 10.0 + (i % 5) as f32 * 7.0).collect();
        let sums = PrefixSums::build(&h);
        let mut top = 0.0f64;
        for (i, v) in h.iter().enumerate() {
            assert_eq!(sums.index_at(top), i, "row top {i}");
            assert_eq!(sums.index_at(top + *v as f64 * 0.5), i, "row mid {i}");
            top += *v as f64;
        }
        assert_eq!(sums.index_at(top), h.len());
        assert_eq!(sums.index_at(top + 1000.0), h.len());
    }

    #[test]
    fn window_is_bounded_with_overscan() {
        let h = vec![32.0f32; 100_000];
        let sums = PrefixSums::build(&h);
        let w = window_for_viewport(&sums, 100_000, 1_000_000.0, 600.0, 4);
        assert!(w.len() < 40, "{w:?}");
        assert!(w.start > 0 && w.end < 100_000);
        let first_visible = sums.index_at(1_000_000.0);
        assert!(first_visible - w.start <= 4);
    }

    #[test]
    fn large_jump_resolves_without_scan() {
        let h: Vec<f32> = (0..100_000).map(|i| 24.0 + (i % 11) as f32).collect();
        let sums = PrefixSums::build(&h);
        let total = sums.total() as f32;
        let w = window_for_viewport(&sums, 100_000, total - 600.0, 600.0, 4);
        assert!(w.end == 100_000);
        assert!(w.len() < 60, "{w:?}");
    }

    #[test]
    fn zero_viewport_mounts_minimal_window() {
        let h = vec![32.0f32; 10_000];
        let sums = PrefixSums::build(&h);
        for h in [0.0, -10.0, f32::NAN] {
            let w = window_for_viewport(&sums, 10_000, 0.0, h, 4);
            assert!(w.len() <= 9, "{w:?} for viewport {h}");
            assert!(!w.is_empty());
        }
    }

    #[test]
    fn point_update_keeps_sums_exact() {
        let mut h = vec![32.0f32; 1000];
        let mut sums = PrefixSums::build(&h);
        h[500] = 96.0;
        sums.add(500, 64.0);
        assert!((sums.total() - (1000.0 * 32.0 + 64.0)).abs() < 1e-6);
        assert_eq!(sums.index_at(500.0 * 32.0 + 1.0), 500);
        assert_eq!(sums.index_at(500.0 * 32.0 + 96.0 + 1.0), 501);
    }

    #[test]
    fn fractional_bottom_row_is_included() {
        let h = vec![10.5f32; 100];
        let sums = PrefixSums::build(&h);
        let w = window_for_viewport(&sums, 100, 0.0, 100.25, 0);
        assert!(w.contains(9), "{w:?}");
        let w2 = window_for_viewport(&sums, 100, 0.0, 94.5, 0);
        assert!(w2.contains(8) && w2.contains(9), "{w2:?}");
    }

    #[test]
    fn nonfinite_scroll_inputs_are_sanitized() {
        let h = vec![32.0f32; 1000];
        let sums = PrefixSums::build(&h);
        let top = window_for_viewport(&sums, 1000, 0.0, 600.0, 4);
        assert_eq!(window_for_viewport(&sums, 1000, f32::NAN, 600.0, 4), top);
        assert_eq!(
            window_for_viewport(&sums, 1000, f32::NEG_INFINITY, 600.0, 4),
            top
        );
        let bottom = window_for_viewport(&sums, 1000, f32::INFINITY, 600.0, 4);
        assert_eq!(bottom.end, 1000);
        assert!(bottom.len() < 40, "{bottom:?}");
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -100.0] {
            let w = window_for_viewport(&sums, 1000, 12345.0, bad, 4);
            assert!(w.len() <= 9 && !w.is_empty(), "{w:?} for {bad}");
        }
    }

    #[test]
    fn huge_overscan_saturates_without_overflow() {
        let h = vec![32.0f32; 100];
        let sums = PrefixSums::build(&h);
        let w = window_for_viewport(&sums, 100, 0.0, 600.0, usize::MAX);
        assert_eq!((w.start, w.end), (0, 100));
        let w2 = window_for_viewport(&sums, 100, 10_000.0, 0.0, usize::MAX);
        assert_eq!((w2.start, w2.end), (0, 100));
    }
}
