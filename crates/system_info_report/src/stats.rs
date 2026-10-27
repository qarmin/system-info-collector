/// Distribution of a series of values.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct Stats {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
    /// Nearest-rank 95th percentile.
    pub p95: f64,
    pub stddev: f64,
}

impl Stats {
    /// `None` for an empty series; NaN values are ignored.
    pub fn of(values: impl IntoIterator<Item = f64>) -> Option<Self> {
        let mut sorted: Vec<f64> = values.into_iter().filter(|v| !v.is_nan()).collect();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_by(f64::total_cmp);
        let count = sorted.len();
        let mean = sorted.iter().sum::<f64>() / count as f64;
        let variance = sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / count as f64;
        let median = if count % 2 == 1 {
            sorted[count / 2]
        } else {
            f64::midpoint(sorted[count / 2 - 1], sorted[count / 2])
        };
        Some(Self {
            count,
            min: sorted[0],
            max: sorted[count - 1],
            mean,
            median,
            p95: sorted[((count as f64 * 0.95).ceil() as usize).clamp(1, count) - 1],
            stddev: variance.sqrt(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn describes_odd_and_even_series() {
        let odd = Stats::of([3.0, 1.0, 2.0]).expect("non-empty");
        assert!(close(odd.median, 2.0) && close(odd.min, 1.0) && close(odd.max, 3.0) && close(odd.mean, 2.0));
        let even = Stats::of([4.0, 1.0, 3.0, 2.0]).expect("non-empty");
        assert!(close(even.median, 2.5));
        assert!(close(even.stddev, 1.25f64.sqrt()));
    }

    #[test]
    fn p95_is_nearest_rank() {
        let stats = Stats::of((1..=100).map(f64::from)).expect("non-empty");
        assert!(close(stats.p95, 95.0));
        assert!(close(Stats::of([7.0]).expect("non-empty").p95, 7.0));
    }

    #[test]
    fn empty_and_nan_only_series_have_no_stats() {
        assert!(Stats::of([]).is_none());
        assert!(Stats::of([f64::NAN]).is_none());
        assert_eq!(Stats::of([f64::NAN, 5.0]).map(|s| s.count), Some(1));
    }
}
