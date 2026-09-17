use crate::{
    motion_filtering::{
        estimator::{angular_error_rad, AngleEstimate},
        report::{
            summarize, DtStatistics, MetricValue, PhysicalProxyMetrics, TiltGroundTruthMetrics,
        },
        synthetic::{FixtureEvent, GroundTruthSample, SyntheticFixture, TiltAxis},
    },
    protocol::Vector3,
};

#[derive(Debug, Clone, Default)]
pub struct GroundTruthMetricAccumulator {
    angular_errors_deg: Vec<f64>,
    roll_errors_deg: Vec<f64>,
    pitch_errors_deg: Vec<f64>,
    tilt_magnitude_deg: Vec<f64>,
    final_drifts_deg: Vec<f64>,
}

impl GroundTruthMetricAccumulator {
    pub fn push_fixture(&mut self, fixture: &SyntheticFixture, estimates: &[AngleEstimate]) {
        for (estimate, truth) in estimates.iter().zip(&fixture.truth) {
            self.angular_errors_deg.push(
                angular_error_rad(&estimate.gravity_direction, &truth.gravity_direction)
                    .to_degrees(),
            );
            self.roll_errors_deg.push(
                wrap_pi(estimate.roll_rad - truth.roll_rad)
                    .to_degrees()
                    .abs(),
            );
            self.pitch_errors_deg.push(
                wrap_pi(estimate.pitch_rad - truth.pitch_rad)
                    .to_degrees()
                    .abs(),
            );
            self.tilt_magnitude_deg.push(
                angular_error_rad(
                    &estimate.gravity_direction,
                    &Vector3 {
                        x: 0.0,
                        y: 0.0,
                        z: -1.0,
                    },
                )
                .to_degrees(),
            );
        }
        if let (Some(estimate), Some(truth)) = (estimates.last(), fixture.truth.last()) {
            self.final_drifts_deg.push(
                angular_error_rad(&estimate.gravity_direction, &truth.gravity_direction)
                    .to_degrees(),
            );
        }
    }

    pub fn summarize(&self) -> TiltGroundTruthMetrics {
        let (armse, ap95, amax) = summarize(&self.angular_errors_deg);
        let (rrmse, _, rmax) = summarize(&self.roll_errors_deg);
        let (prmse, _, pmax) = summarize(&self.pitch_errors_deg);
        TiltGroundTruthMetrics {
            angular_error_rmse_deg: armse,
            angular_error_p95_deg: ap95,
            angular_error_max_deg: amax,
            roll_rmse_deg: rrmse,
            pitch_rmse_deg: prmse,
            roll_max_abs_error_deg: rmax,
            pitch_max_abs_error_deg: pmax,
            tilt_magnitude_rms_deg: rms_metric(&self.tilt_magnitude_deg),
            tilt_peak_to_peak_deg: peak_to_peak_metric(&self.tilt_magnitude_deg),
            final_drift_deg: max_metric(&self.final_drifts_deg),
            overshoot_deg: MetricValue::unavailable("available per fixture only"),
            settling_time_seconds: MetricValue::unavailable("available per fixture only"),
            sine_lag_seconds: MetricValue::unavailable("available per fixture only"),
            recovery_time_seconds: MetricValue::unavailable("available per fixture only"),
        }
    }
}

pub fn dt_statistics(values: &[f64]) -> DtStatistics {
    if values.is_empty() {
        return DtStatistics {
            count: 0,
            min_seconds: MetricValue::unavailable("first sample has no dt"),
            mean_seconds: MetricValue::unavailable("first sample has no dt"),
            p95_seconds: MetricValue::unavailable("first sample has no dt"),
            max_seconds: MetricValue::unavailable("first sample has no dt"),
        };
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    DtStatistics {
        count: values.len(),
        min_seconds: MetricValue::available(sorted[0]),
        mean_seconds: MetricValue::available(values.iter().sum::<f64>() / values.len() as f64),
        p95_seconds: MetricValue::available(percentile_r7(&sorted, 0.95)),
        max_seconds: MetricValue::available(*sorted.last().unwrap()),
    }
}

pub fn evaluate_with_ground_truth(
    fixture: &SyntheticFixture,
    estimates: &[AngleEstimate],
) -> TiltGroundTruthMetrics {
    let mut accumulator = GroundTruthMetricAccumulator::default();
    accumulator.push_fixture(fixture, estimates);
    let mut metrics = accumulator.summarize();
    metrics.overshoot_deg = overshoot(fixture, estimates);
    metrics.settling_time_seconds = settling_time(fixture, estimates);
    metrics.sine_lag_seconds = sine_lag(fixture, estimates);
    metrics.recovery_time_seconds = recovery_time(fixture, estimates);
    metrics
}

pub fn evaluate_physical_proxy(estimates: &[AngleEstimate], dt: &[f64]) -> PhysicalProxyMetrics {
    if estimates.is_empty() {
        return PhysicalProxyMetrics {
            angular_dispersion_from_mean_deg: MetricValue::unavailable("no samples"),
            angular_rate_rms_deg_s: MetricValue::unavailable("no samples"),
            peak_tilt_from_initial_deg: MetricValue::unavailable("no samples"),
            final_to_initial_deg: MetricValue::unavailable("no samples"),
            candidate_divergence_mean_deg: MetricValue::unavailable("computed across candidates"),
            ground_truth_accuracy_metrics: MetricValue::unavailable(
                "ground truth unavailable for physical datasets",
            ),
        };
    }
    let mean = normalize(&estimates.iter().fold(
        Vector3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        |acc, e| Vector3 {
            x: acc.x + e.gravity_direction.x,
            y: acc.y + e.gravity_direction.y,
            z: acc.z + e.gravity_direction.z,
        },
    ));
    let dispersion = (estimates
        .iter()
        .map(|e| {
            angular_error_rad(&e.gravity_direction, &mean)
                .to_degrees()
                .powi(2)
        })
        .sum::<f64>()
        / estimates.len() as f64)
        .sqrt();
    let initial = &estimates[0].gravity_direction;
    let peak = estimates
        .iter()
        .map(|e| angular_error_rad(&e.gravity_direction, initial).to_degrees())
        .fold(0.0, f64::max);
    let final_initial =
        angular_error_rad(&estimates.last().unwrap().gravity_direction, initial).to_degrees();
    let rates = estimates
        .windows(2)
        .zip(dt)
        .filter(|(_, dt)| **dt > 0.0)
        .map(|(pair, dt)| {
            angular_error_rad(&pair[0].gravity_direction, &pair[1].gravity_direction).to_degrees()
                / dt
        })
        .collect::<Vec<_>>();
    PhysicalProxyMetrics {
        angular_dispersion_from_mean_deg: MetricValue::available(dispersion),
        angular_rate_rms_deg_s: rms_metric(&rates),
        peak_tilt_from_initial_deg: MetricValue::available(peak),
        final_to_initial_deg: MetricValue::available(final_initial),
        candidate_divergence_mean_deg: MetricValue::unavailable("computed across candidates"),
        ground_truth_accuracy_metrics: MetricValue::unavailable(
            "ground truth unavailable for physical datasets",
        ),
    }
}

fn overshoot(fixture: &SyntheticFixture, estimates: &[AngleEstimate]) -> MetricValue {
    let Some(FixtureEvent::Step {
        axis,
        onset_seconds,
        initial_angle_rad,
        target_angle_rad,
        ..
    }) = fixture.event.as_ref()
    else {
        return MetricValue::unavailable("not a step fixture");
    };
    let delta = wrap_pi(target_angle_rad - initial_angle_rad);
    if delta.abs() <= 1e-12 {
        return MetricValue::unavailable("step delta is degenerate");
    }
    let direction = delta.signum();
    let mut max_excess = 0.0_f64;
    for (time, estimate) in fixture.elapsed_seconds.iter().zip(estimates) {
        if *time >= *onset_seconds {
            let output = angle_for_axis(estimate, *axis);
            max_excess = max_excess.max(direction * wrap_pi(output - target_angle_rad));
        }
    }
    MetricValue::available(max_excess.max(0.0).to_degrees())
}

fn settling_time(fixture: &SyntheticFixture, estimates: &[AngleEstimate]) -> MetricValue {
    let Some(FixtureEvent::Step {
        axis,
        onset_seconds,
        target_angle_rad,
        settling_band_deg,
        ..
    }) = fixture.event.as_ref()
    else {
        return MetricValue::unavailable("not a step fixture");
    };
    let band = settling_band_deg.to_radians();
    for (index, time) in fixture.elapsed_seconds.iter().enumerate() {
        if *time < *onset_seconds {
            continue;
        }
        if estimates[index..].iter().all(|estimate| {
            wrap_pi(angle_for_axis(estimate, *axis) - target_angle_rad).abs() <= band
        }) {
            return MetricValue::available(*time - onset_seconds);
        }
    }
    MetricValue::unavailable("did not settle within band for hold segment")
}

fn sine_lag(fixture: &SyntheticFixture, estimates: &[AngleEstimate]) -> MetricValue {
    let Some(FixtureEvent::Sine {
        axis,
        start_seconds,
        end_seconds,
        frequency_hz,
    }) = fixture.event.as_ref()
    else {
        return MetricValue::unavailable("not a sine fixture");
    };
    let omega = 2.0 * std::f64::consts::PI * frequency_hz;
    let mut truth_values = Vec::new();
    let mut estimate_values = Vec::new();
    let mut times = Vec::new();
    for ((time, truth), estimate) in fixture
        .elapsed_seconds
        .iter()
        .zip(&fixture.truth)
        .zip(estimates)
    {
        if *time >= *start_seconds && *time <= *end_seconds {
            times.push(*time);
            truth_values.push(truth_angle_for_axis(truth, *axis));
            estimate_values.push(angle_for_axis(estimate, *axis));
        }
    }
    let Some(phase_truth) = fundamental_phase(&times, &truth_values, omega) else {
        return MetricValue::unavailable("truth sine amplitude is degenerate");
    };
    let Some(phase_estimate) = fundamental_phase(&times, &estimate_values, omega) else {
        return MetricValue::unavailable("estimate sine amplitude is degenerate");
    };
    let lag = wrap_pi(phase_truth - phase_estimate) / omega;
    let period = 1.0 / frequency_hz;
    if lag.abs() > period * 0.5 + 1e-9 {
        return MetricValue::unavailable("lag exceeds half-period after phase wrapping");
    }
    MetricValue::available(lag)
}

fn recovery_time(fixture: &SyntheticFixture, estimates: &[AngleEstimate]) -> MetricValue {
    let Some(FixtureEvent::GravityContamination {
        end_seconds,
        recovery_band_deg,
        minimum_recovery_hold_seconds,
        ..
    }) = fixture.event.as_ref()
    else {
        return MetricValue::unavailable("not a gravity contamination fixture");
    };
    let band = recovery_band_deg.to_radians();
    for (index, time) in fixture.elapsed_seconds.iter().enumerate() {
        if *time <= *end_seconds {
            continue;
        }
        let hold_until = *time + minimum_recovery_hold_seconds;
        let has_temporal_coverage = fixture.elapsed_seconds[index..]
            .iter()
            .any(|sample_time| *sample_time >= hold_until);
        if !has_temporal_coverage {
            continue;
        }
        let ok = estimates[index..]
            .iter()
            .zip(&fixture.truth[index..])
            .zip(&fixture.elapsed_seconds[index..])
            .take_while(|(_, t)| **t <= hold_until)
            .all(|((estimate, truth), _)| {
                angular_error_rad(&estimate.gravity_direction, &truth.gravity_direction) <= band
            });
        if ok {
            return MetricValue::available(*time - end_seconds);
        }
    }
    MetricValue::unavailable("did not recover within band for required hold")
}

fn fundamental_phase(times: &[f64], values: &[f64], omega: f64) -> Option<f64> {
    if values.len() < 3 {
        return None;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let mut sin_projection = 0.0;
    let mut cos_projection = 0.0;
    for (time, value) in times.iter().zip(values) {
        let centered = value - mean;
        sin_projection += centered * (omega * time).sin();
        cos_projection += centered * (omega * time).cos();
    }
    let amplitude = (sin_projection * sin_projection + cos_projection * cos_projection).sqrt();
    if amplitude <= 1e-9 {
        None
    } else {
        Some(cos_projection.atan2(sin_projection))
    }
}

pub fn percentile_r7(sorted: &[f64], p: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = p.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
    }
}
pub fn wrap_pi(mut value: f64) -> f64 {
    while value > std::f64::consts::PI {
        value -= 2.0 * std::f64::consts::PI;
    }
    while value < -std::f64::consts::PI {
        value += 2.0 * std::f64::consts::PI;
    }
    value
}
fn angle_for_axis(estimate: &AngleEstimate, axis: TiltAxis) -> f64 {
    match axis {
        TiltAxis::Roll => estimate.roll_rad,
        TiltAxis::Pitch => estimate.pitch_rad,
    }
}
fn truth_angle_for_axis(truth: &GroundTruthSample, axis: TiltAxis) -> f64 {
    match axis {
        TiltAxis::Roll => truth.roll_rad,
        TiltAxis::Pitch => truth.pitch_rad,
    }
}
fn rms_metric(values: &[f64]) -> MetricValue {
    if values.is_empty() {
        MetricValue::unavailable("no samples")
    } else {
        MetricValue::available(
            (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt(),
        )
    }
}
fn max_metric(values: &[f64]) -> MetricValue {
    values
        .iter()
        .copied()
        .reduce(f64::max)
        .map(MetricValue::available)
        .unwrap_or_else(|| MetricValue::unavailable("no samples"))
}
fn peak_to_peak_metric(values: &[f64]) -> MetricValue {
    if values.is_empty() {
        MetricValue::unavailable("no samples")
    } else {
        MetricValue::available(
            values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                - values.iter().copied().fold(f64::INFINITY, f64::min),
        )
    }
}
fn normalize(v: &Vector3) -> Vector3 {
    let m = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if m <= 1e-12 {
        Vector3 {
            x: 0.0,
            y: 0.0,
            z: -1.0,
        }
    } else {
        Vector3 {
            x: v.x / m,
            y: v.y / m,
            z: v.z / m,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion_filtering::synthetic::synthetic_suite;

    #[test]
    fn r7_matches_b1_example() {
        assert_eq!(percentile_r7(&[100000.0, 200000.0], 0.95), 195000.0);
    }
    #[test]
    fn wrap_metric_handles_pi() {
        assert!((wrap_pi(3.5) - (-2.7831853071795862)).abs() < 1e-12);
    }

    #[test]
    fn baseline_step_overshoot_is_zero_and_settling_is_relative() {
        let suite = synthetic_suite().unwrap();
        let fixture = suite
            .iter()
            .find(|f| f.name == "roll_step_positive")
            .unwrap();
        let estimates = fixture
            .truth
            .iter()
            .map(|truth| AngleEstimate {
                gravity_direction: truth.gravity_direction.clone(),
                roll_rad: truth.roll_rad,
                pitch_rad: truth.pitch_rad,
                yaw_available: false,
            })
            .collect::<Vec<_>>();
        let metrics = evaluate_with_ground_truth(fixture, &estimates);
        assert_metric_close(&metrics.overshoot_deg, 0.0, 1e-9);
        assert_metric_close(&metrics.settling_time_seconds, 0.004201680672268893, 0.02);
    }

    #[test]
    fn baseline_sine_lag_is_zero_and_recovery_is_bounded() {
        let suite = synthetic_suite().unwrap();
        let sine = suite.iter().find(|f| f.name == "roll_sine").unwrap();
        let sine_estimates = truth_estimates(sine);
        assert_metric_close(
            &evaluate_with_ground_truth(sine, &sine_estimates).sine_lag_seconds,
            0.0,
            1e-9,
        );
        let pulse = suite.iter().find(|f| f.name == "gravity_pulse").unwrap();
        let pulse_estimates = truth_estimates(pulse);
        let recovery = evaluate_with_ground_truth(pulse, &pulse_estimates)
            .recovery_time_seconds
            .value()
            .unwrap();
        assert!((0.0..=1.8).contains(&recovery));
    }

    #[test]
    fn sine_lag_reports_known_non_zero_lag_and_zero_lag() {
        let frequency_hz = 1.0;
        let lag_seconds = 0.125;
        let times = (0..200).map(|i| i as f64 / 100.0).collect::<Vec<_>>();
        let omega = 2.0 * std::f64::consts::PI * frequency_hz;
        let truth_angles = times
            .iter()
            .map(|t| 10_f64.to_radians() * (omega * t).sin())
            .collect::<Vec<_>>();
        let delayed_angles = times
            .iter()
            .map(|t| 10_f64.to_radians() * (omega * (t - lag_seconds)).sin())
            .collect::<Vec<_>>();
        let fixture = metric_fixture(
            "known_lag",
            times.clone(),
            truth_angles.clone(),
            Some(FixtureEvent::Sine {
                axis: TiltAxis::Roll,
                start_seconds: 0.0,
                end_seconds: 1.99,
                frequency_hz,
            }),
        );

        assert_metric_close(
            &sine_lag(&fixture, &estimates_from_rolls(&delayed_angles)),
            lag_seconds,
            1e-12,
        );
        assert_metric_close(
            &sine_lag(&fixture, &estimates_from_rolls(&truth_angles)),
            0.0,
            1e-12,
        );
    }

    #[test]
    fn sine_lag_degenerate_amplitude_is_unavailable() {
        let times = regular_times(20, 0.0, 1.0);
        let fixture = metric_fixture(
            "degenerate_sine",
            times.clone(),
            vec![0.0; times.len()],
            Some(FixtureEvent::Sine {
                axis: TiltAxis::Roll,
                start_seconds: 0.0,
                end_seconds: 1.0,
                frequency_hz: 1.0,
            }),
        );
        assert_unavailable(&sine_lag(
            &fixture,
            &estimates_from_rolls(&vec![0.0; times.len()]),
        ));
    }

    #[test]
    fn step_overshoot_handles_positive_and_no_overshoot_cases() {
        let times = vec![0.0, 1.0, 1.1, 1.2, 1.3];
        let target = 10_f64.to_radians();
        let fixture = metric_fixture(
            "step",
            times,
            vec![0.0, target, target, target, target],
            Some(FixtureEvent::Step {
                axis: TiltAxis::Roll,
                onset_seconds: 1.0,
                initial_angle_rad: 0.0,
                target_angle_rad: target,
                settling_band_deg: 1.0,
            }),
        );
        assert_metric_close(
            &overshoot(
                &fixture,
                &estimates_from_rolls(&[
                    0.0,
                    9_f64.to_radians(),
                    12_f64.to_radians(),
                    11_f64.to_radians(),
                    target,
                ]),
            ),
            2.0,
            1e-12,
        );
        assert_metric_close(
            &overshoot(
                &fixture,
                &estimates_from_rolls(&[
                    0.0,
                    8_f64.to_radians(),
                    9_f64.to_radians(),
                    9.5_f64.to_radians(),
                    target,
                ]),
            ),
            0.0,
            1e-12,
        );
    }

    #[test]
    fn settling_time_reports_known_time_and_never_settles() {
        let target = 10_f64.to_radians();
        let fixture = metric_fixture(
            "settling",
            vec![0.0, 1.0, 1.5, 2.0, 2.5],
            vec![0.0, target, target, target, target],
            Some(FixtureEvent::Step {
                axis: TiltAxis::Roll,
                onset_seconds: 1.0,
                initial_angle_rad: 0.0,
                target_angle_rad: target,
                settling_band_deg: 1.0,
            }),
        );
        assert_metric_close(
            &settling_time(
                &fixture,
                &estimates_from_rolls(&[
                    0.0,
                    8_f64.to_radians(),
                    9.2_f64.to_radians(),
                    10.5_f64.to_radians(),
                    10_f64.to_radians(),
                ]),
            ),
            0.5,
            1e-12,
        );
        assert_unavailable(&settling_time(
            &fixture,
            &estimates_from_rolls(&[
                0.0,
                12_f64.to_radians(),
                12_f64.to_radians(),
                12_f64.to_radians(),
                12_f64.to_radians(),
            ]),
        ));
    }

    #[test]
    fn recovery_time_requires_only_the_configured_hold_window() {
        let fixture = recovery_fixture(vec![0.0, 1.0, 1.1, 1.2, 1.3, 1.5, 1.8]);
        assert_metric_close(
            &recovery_time(
                &fixture,
                &estimates_with_roll_error_deg(&[0.0, 20.0, 5.0, 0.5, 0.2, 0.4, 8.0]),
            ),
            0.2,
            1e-12,
        );
    }

    #[test]
    fn recovery_time_rejects_hold_break_insufficient_coverage_and_no_recovery() {
        let fixture = recovery_fixture(vec![0.0, 1.0, 1.1, 1.2, 1.3, 1.5]);
        assert_unavailable(&recovery_time(
            &fixture,
            &estimates_with_roll_error_deg(&[0.0, 20.0, 0.5, 2.0, 0.4, 0.3]),
        ));

        let short_fixture = recovery_fixture(vec![0.0, 1.0, 1.1, 1.2]);
        assert_unavailable(&recovery_time(
            &short_fixture,
            &estimates_with_roll_error_deg(&[0.0, 20.0, 0.5, 0.4]),
        ));

        assert_unavailable(&recovery_time(
            &fixture,
            &estimates_with_roll_error_deg(&[0.0, 20.0, 3.0, 2.0, 4.0, 5.0]),
        ));
    }

    #[test]
    fn temporal_metrics_are_unavailable_without_required_event() {
        let fixture = metric_fixture("no_event", vec![0.0, 1.0, 2.0], vec![0.0, 0.0, 0.0], None);
        let estimates = estimates_from_rolls(&[0.0, 0.0, 0.0]);
        assert_unavailable(&overshoot(&fixture, &estimates));
        assert_unavailable(&settling_time(&fixture, &estimates));
        assert_unavailable(&sine_lag(&fixture, &estimates));
        assert_unavailable(&recovery_time(&fixture, &estimates));
    }

    fn truth_estimates(fixture: &SyntheticFixture) -> Vec<AngleEstimate> {
        fixture
            .truth
            .iter()
            .map(|truth| AngleEstimate {
                gravity_direction: truth.gravity_direction.clone(),
                roll_rad: truth.roll_rad,
                pitch_rad: truth.pitch_rad,
                yaw_available: false,
            })
            .collect()
    }
    fn assert_metric_close(metric: &MetricValue, expected: f64, tolerance: f64) {
        let actual = metric.value().expect("metric available");
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected}, got {actual}"
        );
    }

    fn assert_unavailable(metric: &MetricValue) {
        assert!(
            metric.value().is_none(),
            "metric should be unavailable: {metric:?}"
        );
    }

    fn metric_fixture(
        name: &str,
        times: Vec<f64>,
        truth_rolls: Vec<f64>,
        event: Option<FixtureEvent>,
    ) -> SyntheticFixture {
        let template = synthetic_suite().unwrap().remove(0);
        let truth = truth_rolls
            .iter()
            .map(|roll| GroundTruthSample {
                gravity_direction: direction_from_roll(*roll),
                roll_rad: *roll,
                pitch_rad: 0.0,
            })
            .collect::<Vec<_>>();
        SyntheticFixture {
            name: name.to_owned(),
            raw_samples: Vec::new(),
            calibration_profile: template.calibration_profile,
            calibration_kind: "test".to_owned(),
            truth,
            elapsed_seconds: times,
            event,
            seed: None,
            canonical_samples: Vec::new(),
        }
    }

    fn recovery_fixture(times: Vec<f64>) -> SyntheticFixture {
        metric_fixture(
            "recovery",
            times.clone(),
            vec![0.0; times.len()],
            Some(FixtureEvent::GravityContamination {
                start_seconds: 0.8,
                end_seconds: 1.0,
                recovery_band_deg: 1.0,
                minimum_recovery_hold_seconds: 0.3,
            }),
        )
    }

    fn estimates_with_roll_error_deg(errors_deg: &[f64]) -> Vec<AngleEstimate> {
        estimates_from_rolls(
            &errors_deg
                .iter()
                .map(|deg| deg.to_radians())
                .collect::<Vec<_>>(),
        )
    }

    fn estimates_from_rolls(rolls: &[f64]) -> Vec<AngleEstimate> {
        rolls
            .iter()
            .map(|roll| AngleEstimate {
                gravity_direction: direction_from_roll(*roll),
                roll_rad: *roll,
                pitch_rad: 0.0,
                yaw_available: false,
            })
            .collect()
    }

    fn direction_from_roll(roll: f64) -> Vector3 {
        Vector3 {
            x: roll.sin(),
            y: 0.0,
            z: -roll.cos(),
        }
    }

    fn regular_times(samples: usize, start: f64, end: f64) -> Vec<f64> {
        (0..samples)
            .map(|i| start + (end - start) * i as f64 / (samples - 1) as f64)
            .collect()
    }
}
