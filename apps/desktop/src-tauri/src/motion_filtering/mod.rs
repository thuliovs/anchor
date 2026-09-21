pub mod estimator;
pub mod metrics;
pub mod report;
pub mod selection;
pub mod synthetic;

use crate::{
    calibration::{
        apply_calibration_to_angular, apply_calibration_to_gravity, apply_calibration_to_linear,
        load_calibration_profile_file, CalibrationError, CalibrationProfileV1,
    },
    dataset::{analyze_dataset_file, load_validated_dataset_file, DatasetAnalysisError},
    motion_filtering::{
        estimator::{
            AngleEstimate, CalibratedMotionSample, ComplementaryTiltEstimator, Estimator,
            GravityOnlyEstimator, LowPassGravityEstimator,
        },
        metrics::GroundTruthMetricAccumulator,
        metrics::{dt_statistics, evaluate_physical_proxy, evaluate_with_ground_truth},
        report::{
            CalibrationInputReport, CandidateReport, CandidateReportMetrics, CandidateRun,
            EvaluationInput, EvaluationReport, FixtureCandidateReport, InputKind, MetricValue,
            PhysicalProxyMetrics, SyntheticFixtureSummary,
        },
        synthetic::{synthetic_suite, SyntheticFixture},
    },
};
use serde::{Deserialize, Serialize};
use std::{fmt, io, path::Path};

pub const EVALUATION_REPORT_VERSION: u8 = 1;
pub const DEFAULT_LOW_PASS_TAU_MS: &[f64] = &[50.0, 100.0, 200.0, 400.0];
pub const DEFAULT_COMPLEMENTARY_TAU_MS: &[f64] = &[100.0, 250.0, 500.0, 1000.0];

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluationConfig {
    pub low_pass_tau_ms: Vec<f64>,
    pub complementary_tau_ms: Vec<f64>,
}

impl EvaluationConfig {
    pub fn new(
        low_pass_tau_ms: Vec<f64>,
        complementary_tau_ms: Vec<f64>,
    ) -> Result<Self, EvaluationError> {
        validate_tau_list(&low_pass_tau_ms, "lowPassTauMs")?;
        validate_tau_list(&complementary_tau_ms, "complementaryCorrectionTauMs")?;
        Ok(Self {
            low_pass_tau_ms,
            complementary_tau_ms,
        })
    }
}

impl Default for EvaluationConfig {
    fn default() -> Self {
        Self {
            low_pass_tau_ms: DEFAULT_LOW_PASS_TAU_MS.to_vec(),
            complementary_tau_ms: DEFAULT_COMPLEMENTARY_TAU_MS.to_vec(),
        }
    }
}

#[derive(Debug)]
pub enum EvaluationError {
    Dataset(DatasetAnalysisError),
    Calibration(CalibrationError),
    InvalidConfig(String),
    InvalidInput(String),
    Estimator(estimator::EstimatorError),
    Io(io::Error),
}

impl fmt::Display for EvaluationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dataset(err) => write!(f, "{err}"),
            Self::Calibration(err) => write!(f, "{err}"),
            Self::InvalidConfig(msg) | Self::InvalidInput(msg) => f.write_str(msg),
            Self::Estimator(err) => write!(f, "{err}"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for EvaluationError {}
impl From<DatasetAnalysisError> for EvaluationError {
    fn from(value: DatasetAnalysisError) -> Self {
        Self::Dataset(value)
    }
}
impl From<CalibrationError> for EvaluationError {
    fn from(value: CalibrationError) -> Self {
        Self::Calibration(value)
    }
}
impl From<estimator::EstimatorError> for EvaluationError {
    fn from(value: estimator::EstimatorError) -> Self {
        Self::Estimator(value)
    }
}
impl From<io::Error> for EvaluationError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TauGrid {
    pub low_pass_tau_ms: Vec<f64>,
    pub complementary_correction_tau_ms: Vec<f64>,
}

pub fn evaluate_synthetic_suite(
    config: &EvaluationConfig,
) -> Result<EvaluationReport, EvaluationError> {
    let fixtures = synthetic_suite()?;
    let mut configuration_reports = Vec::new();
    let mut fixture_summaries = Vec::new();
    let mut all_dt = Vec::new();

    for fixture in &fixtures {
        all_dt.extend(fixture.dt_seconds());
        fixture_summaries.push(SyntheticFixtureSummary {
            name: fixture.name.clone(),
            sample_count: fixture.raw_samples.len(),
            ground_truth_available: true,
            calibration_kind: fixture.calibration_kind.clone(),
            calibration_profile_version: fixture.calibration_profile.calibration_profile_version,
            seed: fixture.seed,
        });
    }

    for run in candidate_runs(config)? {
        let mut accumulator = GroundTruthMetricAccumulator::default();
        let mut fixture_results = Vec::new();
        for fixture in &fixtures {
            let estimates = run_estimator_for_fixture(&run, fixture)?;
            accumulator.push_fixture(fixture, &estimates);
            fixture_results.push(FixtureCandidateReport {
                fixture: fixture.name.clone(),
                metrics: evaluate_with_ground_truth(fixture, &estimates),
            });
        }
        configuration_reports.push(CandidateReport {
            candidate: run.candidate_id(),
            parameters: run.parameters(),
            summary: CandidateReportMetrics::Synthetic(accumulator.summarize()),
            fixture_results,
        });
    }

    Ok(EvaluationReport {
        evaluation_report_version: EVALUATION_REPORT_VERSION,
        input: EvaluationInput {
            kind: InputKind::SyntheticSuite,
            identifier: "synthetic-suite-v1".to_owned(),
            profile_identifier: None,
            synthetic_fixtures: fixture_summaries,
        },
        ground_truth_available: true,
        yaw_calibrated: false,
        sample_count: fixtures.iter().map(|f| f.raw_samples.len()).sum(),
        observed_duration_us: fixtures
            .iter()
            .map(SyntheticFixture::observed_duration_us)
            .sum(),
        dt_statistics: dt_statistics(&all_dt),
        calibration: CalibrationInputReport::SyntheticSuiteMixed,
        configurations: configuration_reports,
        warnings: vec![
            "yaw is not observable and is not estimated".to_owned(),
            "no automatic winner is selected in B3a".to_owned(),
        ],
        limitations: common_limitations(true),
    })
}

pub fn evaluate_dataset_file(
    dataset_path: &Path,
    profile_path: &Path,
    config: &EvaluationConfig,
) -> Result<EvaluationReport, EvaluationError> {
    let dataset = load_validated_dataset_file(dataset_path)?;
    let analysis = analyze_dataset_file(dataset_path)?;
    if analysis.session_count != 1 {
        return Err(EvaluationError::InvalidInput(format!(
            "evaluation requires exactly one session, got {}",
            analysis.session_count
        )));
    }
    let profile = load_calibration_profile_file(profile_path)?;
    if profile.yaw_calibrated {
        return Err(EvaluationError::InvalidInput(
            "yawCalibrated must remain false".to_owned(),
        ));
    }

    let samples = dataset
        .samples
        .iter()
        .map(|record| calibrate_sample(&profile, &record.sample))
        .collect::<Result<Vec<_>, _>>()?;
    let dt = dataset
        .samples
        .windows(2)
        .map(|pair| {
            (pair[1].sample.session_elapsed_us - pair[0].sample.session_elapsed_us) as f64
                / 1_000_000.0
        })
        .collect::<Vec<_>>();
    let mut configuration_reports = Vec::new();
    let mut candidate_outputs: Vec<(String, Vec<AngleEstimate>)> = Vec::new();
    for run in candidate_runs(config)? {
        let estimates = run_estimator_for_samples(&run, &samples, &dt)?;
        let metrics = evaluate_physical_proxy(&estimates, &dt);
        candidate_outputs.push((run.candidate_id(), estimates));
        configuration_reports.push(CandidateReport {
            candidate: run.candidate_id(),
            parameters: run.parameters(),
            summary: CandidateReportMetrics::Physical(metrics),
            fixture_results: Vec::new(),
        });
    }
    let divergence = candidate_divergence_degrees(&candidate_outputs);
    for report in &mut configuration_reports {
        if let CandidateReportMetrics::Physical(PhysicalProxyMetrics {
            candidate_divergence_mean_deg,
            ..
        }) = &mut report.summary
        {
            *candidate_divergence_mean_deg = MetricValue::available(divergence);
        }
    }

    Ok(EvaluationReport {
        evaluation_report_version: EVALUATION_REPORT_VERSION,
        input: EvaluationInput { kind: InputKind::Dataset, identifier: safe_basename(dataset_path), profile_identifier: Some(safe_basename(profile_path)), synthetic_fixtures: Vec::new() },
        ground_truth_available: false,
        yaw_calibrated: profile.yaw_calibrated,
        sample_count: dataset.samples.len(),
        observed_duration_us: analysis.observed_duration_us,
        dt_statistics: dt_statistics(&dt),
        calibration: CalibrationInputReport::Profile { profile: safe_basename(profile_path), mounting_convention: profile.mounting_convention.clone(), source_dataset: profile.source_dataset.clone() },
        configurations: configuration_reports,
        warnings: vec!["physical datasets have no ground truth; metrics are behavioral proxies, not angular accuracy".to_owned(), "the calibration profile assumes the same phone and preserved mounting across captures".to_owned(), "yaw is not observable and is not estimated".to_owned(), "no automatic winner is selected in B3a".to_owned()],
        limitations: common_limitations(false),
    })
}

pub fn format_human_evaluation(report: &EvaluationReport) -> String {
    report::format_human_report(report)
}

fn candidate_runs(config: &EvaluationConfig) -> Result<Vec<CandidateRun>, EvaluationError> {
    let mut runs = vec![CandidateRun::GravityOnly];
    runs.extend(
        config
            .low_pass_tau_ms
            .iter()
            .copied()
            .map(|tau| CandidateRun::LowPass { tau_ms: tau }),
    );
    runs.extend(config.complementary_tau_ms.iter().copied().map(|tau| {
        CandidateRun::Complementary {
            correction_tau_ms: tau,
        }
    }));
    Ok(runs)
}

fn run_estimator_for_fixture(
    run: &CandidateRun,
    fixture: &SyntheticFixture,
) -> Result<Vec<AngleEstimate>, EvaluationError> {
    let samples = fixture
        .raw_samples
        .iter()
        .map(|sample| calibrate_sample(&fixture.calibration_profile, sample))
        .collect::<Result<Vec<_>, _>>()?;
    run_estimator_for_samples(run, &samples, &fixture.dt_seconds())
}

fn run_estimator_for_samples(
    run: &CandidateRun,
    samples: &[CalibratedMotionSample],
    dt: &[f64],
) -> Result<Vec<AngleEstimate>, EvaluationError> {
    let mut estimator = build_estimator(run)?;
    let mut estimates = Vec::with_capacity(samples.len());
    for (index, sample) in samples.iter().enumerate() {
        let estimate = if index == 0 {
            estimator.initialize(sample)?
        } else {
            estimator.update(sample, dt[index - 1])?
        };
        estimates.push(estimate);
    }
    Ok(estimates)
}

fn build_estimator(run: &CandidateRun) -> Result<Box<dyn Estimator>, EvaluationError> {
    Ok(match *run {
        CandidateRun::GravityOnly => Box::new(GravityOnlyEstimator::new()),
        CandidateRun::LowPass { tau_ms } => {
            Box::new(LowPassGravityEstimator::new(tau_ms / 1000.0)?)
        }
        CandidateRun::Complementary { correction_tau_ms } => {
            Box::new(ComplementaryTiltEstimator::new(correction_tau_ms / 1000.0)?)
        }
    })
}

fn calibrate_sample(
    profile: &CalibrationProfileV1,
    sample: &crate::protocol::MotionSampleV1,
) -> Result<CalibratedMotionSample, EvaluationError> {
    Ok(CalibratedMotionSample {
        gravity_mps2: apply_calibration_to_gravity(profile, &sample.gravity_mps2)?,
        angular_velocity_rad_s: apply_calibration_to_angular(
            profile,
            &sample.angular_velocity_rad_s,
        )?,
        linear_acceleration_mps2: Some(apply_calibration_to_linear(
            profile,
            &sample.linear_acceleration_mps2,
        )?),
    })
}

fn validate_tau_list(values: &[f64], name: &str) -> Result<(), EvaluationError> {
    if values.is_empty() {
        return Err(EvaluationError::InvalidConfig(format!(
            "{name} must not be empty"
        )));
    }
    let mut sorted = values.to_vec();
    for value in &sorted {
        if !value.is_finite() || *value <= 0.0 {
            return Err(EvaluationError::InvalidConfig(format!(
                "{name} values must be positive finite numbers"
            )));
        }
    }
    sorted.sort_by(f64::total_cmp);
    for pair in sorted.windows(2) {
        if pair[0] == pair[1] {
            return Err(EvaluationError::InvalidConfig(format!(
                "{name} must not contain duplicate values"
            )));
        }
    }
    Ok(())
}

fn safe_basename(path: &Path) -> String {
    path.file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("input")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn common_limitations(ground_truth: bool) -> Vec<String> {
    let mut values = vec![
        "Android TYPE_GRAVITY may already include vendor filtering or fusion; baseline means no additional Anchor filter".to_owned(),
        "MotionSampleV1 has no individual timestamps for linear acceleration, gravity and gyroscope".to_owned(),
        "no rigorous physical delay between gravity and gyro can be inferred from this payload".to_owned(),
        "yaw is unavailable because B2 yawCalibrated is false and no magnetometer is used".to_owned(),
        "complementary correction uses normalized vector mixing, not SLERP".to_owned(),
        "adaptive correction from linear acceleration is intentionally out of scope for B3a".to_owned(),
    ];
    if !ground_truth {
        values.push("physical capture metrics are behavioral proxies and must not be interpreted as angular accuracy".to_owned());
    }
    values
}

fn candidate_divergence_degrees(outputs: &[(String, Vec<AngleEstimate>)]) -> f64 {
    if outputs.len() < 2 {
        return 0.0;
    }
    let sample_count = outputs[0].1.len();
    let mut sum = 0.0;
    let mut count = 0.0;
    for i in 0..sample_count {
        for a in 0..outputs.len() {
            for b in (a + 1)..outputs.len() {
                sum += estimator::angular_error_rad(
                    &outputs[a].1[i].gravity_direction,
                    &outputs[b].1[i].gravity_direction,
                )
                .to_degrees();
                count += 1.0;
            }
        }
    }
    if count == 0.0 {
        0.0
    } else {
        sum / count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tau_list_rejects_empty_invalid_and_duplicates() {
        assert!(EvaluationConfig::new(vec![], vec![100.0]).is_err());
        assert!(EvaluationConfig::new(vec![0.0], vec![100.0]).is_err());
        assert!(EvaluationConfig::new(vec![50.0, 50.0], vec![100.0]).is_err());
    }
}
