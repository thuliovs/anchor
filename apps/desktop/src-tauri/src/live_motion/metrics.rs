use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveMotionMetrics {
    pub processed_events: u64,
    pub valid_outputs: u64,
    pub session_resets: u64,
    pub gap_resets: u64,
    pub invalid_dt_resets: u64,
    pub sequence_gap_events: u64,
    pub estimated_missing_samples: u64,
    pub non_monotonic_sequence_events: u64,
    pub calibration_errors: u64,
    pub estimator_errors: u64,
    pub provenance_mismatches: u64,
    pub source_interval_us: OnlineScalarStatsSnapshot,
    pub receive_interarrival_us: OnlineScalarStatsSnapshot,
    pub receive_minus_source_delta_us: OnlineScalarStatsSnapshot,
    pub max_abs_receive_minus_source_delta_us: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveMotionMetricAccumulators {
    pub processed_events: u64,
    pub valid_outputs: u64,
    pub session_resets: u64,
    pub gap_resets: u64,
    pub invalid_dt_resets: u64,
    pub sequence_gap_events: u64,
    pub estimated_missing_samples: u64,
    pub non_monotonic_sequence_events: u64,
    pub calibration_errors: u64,
    pub estimator_errors: u64,
    pub provenance_mismatches: u64,
    pub source_interval_us: OnlineScalarStats,
    pub receive_interarrival_us: OnlineScalarStats,
    pub receive_minus_source_delta_us: OnlineScalarStats,
    pub max_abs_receive_minus_source_delta_us: f64,
}

impl LiveMotionMetricAccumulators {
    pub fn snapshot(&self) -> LiveMotionMetrics {
        LiveMotionMetrics {
            processed_events: self.processed_events,
            valid_outputs: self.valid_outputs,
            session_resets: self.session_resets,
            gap_resets: self.gap_resets,
            invalid_dt_resets: self.invalid_dt_resets,
            sequence_gap_events: self.sequence_gap_events,
            estimated_missing_samples: self.estimated_missing_samples,
            non_monotonic_sequence_events: self.non_monotonic_sequence_events,
            calibration_errors: self.calibration_errors,
            estimator_errors: self.estimator_errors,
            provenance_mismatches: self.provenance_mismatches,
            source_interval_us: self.source_interval_us.snapshot(),
            receive_interarrival_us: self.receive_interarrival_us.snapshot(),
            receive_minus_source_delta_us: self.receive_minus_source_delta_us.snapshot(),
            max_abs_receive_minus_source_delta_us: finite_or_zero(
                self.max_abs_receive_minus_source_delta_us,
            ),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OnlineScalarStats {
    count: u64,
    mean: f64,
    m2: f64,
    min: f64,
    max: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineScalarStatsSnapshot {
    pub count: u64,
    pub mean: f64,
    pub population_stddev: f64,
    pub min: f64,
    pub max: f64,
}

impl OnlineScalarStats {
    pub fn push(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.count += 1;
        if self.count == 1 {
            self.mean = value;
            self.m2 = 0.0;
            self.min = value;
            self.max = value;
            return;
        }
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = value - self.mean;
        self.m2 += delta * delta2;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    pub fn snapshot(&self) -> OnlineScalarStatsSnapshot {
        if self.count == 0 {
            return OnlineScalarStatsSnapshot {
                count: 0,
                mean: 0.0,
                population_stddev: 0.0,
                min: 0.0,
                max: 0.0,
            };
        }
        OnlineScalarStatsSnapshot {
            count: self.count,
            mean: finite_or_zero(self.mean),
            population_stddev: finite_or_zero((self.m2 / self.count as f64).sqrt()),
            min: finite_or_zero(self.min),
            max: finite_or_zero(self.max),
        }
    }
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_stats_compute_population_stddev_min_max() {
        let mut stats = OnlineScalarStats::default();
        for value in [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0] {
            stats.push(value);
        }
        let s = stats.snapshot();
        assert_eq!(s.count, 8);
        assert!((s.mean - 5.0).abs() < 1e-12);
        assert!((s.population_stddev - 2.0).abs() < 1e-12);
        assert_eq!(s.min, 2.0);
        assert_eq!(s.max, 9.0);
    }
}
