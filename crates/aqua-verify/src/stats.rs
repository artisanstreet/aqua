#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Distribution {
    pub count: usize,
    pub min: f64,
    pub mean: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub p999: f64,
    pub max: f64,
}

pub fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let index = ((sorted.len() as f64 - 1.0) * fraction.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

impl Distribution {
    pub fn from_samples(samples: &[f64]) -> Option<Self> {
        let mut sorted: Vec<f64> = samples.iter().copied().filter(|s| s.is_finite()).collect();
        sorted.sort_by(f64::total_cmp);
        let first = *sorted.first()?;
        let last = *sorted.last()?;
        Some(Self {
            count: sorted.len(),
            min: first,
            mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
            p50: percentile(&sorted, 0.5),
            p90: percentile(&sorted, 0.9),
            p99: percentile(&sorted, 0.99),
            p999: percentile(&sorted, 0.999),
            max: last,
        })
    }
}

impl std::fmt::Display for Distribution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "n={} min={:.3} mean={:.3} p50={:.3} p90={:.3} p99={:.3} p99.9={:.3} max={:.3}",
            self.count, self.min, self.mean, self.p50, self.p90, self.p99, self.p999, self.max
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jank {
    pub frames: usize,
    pub over_budget: usize,
    pub longest_run: usize,
    pub worst: f64,
}

impl Jank {
    pub fn over_budget_fraction(self) -> f64 {
        match self.frames {
            0 => 0.0,
            frames => self.over_budget as f64 / frames as f64,
        }
    }
}

pub fn jank(frame_times: &[f64], budget: f64) -> Jank {
    let (over_budget, longest_run, _) = frame_times.iter().fold(
        (0usize, 0usize, 0usize),
        |(over, longest, run), time| match *time > budget {
            true => (over + 1, longest.max(run + 1), run + 1),
            false => (over, longest, 0),
        },
    );
    Jank {
        frames: frame_times.len(),
        over_budget,
        longest_run,
        worst: frame_times.iter().copied().fold(0.0, f64::max),
    }
}

pub fn intervals(timestamps: &[f64]) -> Vec<f64> {
    timestamps
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect()
}
