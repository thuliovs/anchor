use crate::{
    dataset::{load_validated_dataset_file, RecordingScenario},
    motion_filtering::{
        evaluate_dataset_file, evaluate_synthetic_suite, report::MetricValue,
        synthetic::synthetic_suite, EvaluationConfig, EvaluationError,
        DEFAULT_COMPLEMENTARY_TAU_MS, DEFAULT_LOW_PASS_TAU_MS,
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, path::PathBuf};

pub const SELECTION_REPORT_VERSION: u8 = 1;
pub const POLICY_VERSION: u8 = 1;
pub const POLICY_SOURCE: &str = "b3b_offline_selection";
pub const DEFAULT_SELECTION_LOW_PASS_TAU_MS: &[f64] = &[
    25.0, 50.0, 75.0, 100.0, 150.0, 200.0, 300.0, 400.0, 600.0, 800.0,
];
pub const DEFAULT_SELECTION_COMPLEMENTARY_TAU_MS: &[f64] = &[
    50.0, 75.0, 100.0, 150.0, 250.0, 400.0, 600.0, 1000.0, 1500.0, 2000.0,
];

const TOLERANCES: SelectionTolerances = SelectionTolerances {
    sine_lag_seconds: 0.001,
    settling_recovery_seconds: 0.01,
    angular_error_deg: 0.01,
    angular_proxy_deg: 0.01,
    angular_rate_proxy_deg_s: 0.01,
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionTolerances {
    pub sine_lag_seconds: f64,
    pub settling_recovery_seconds: f64,
    pub angular_error_deg: f64,
    pub angular_proxy_deg: f64,
    pub angular_rate_proxy_deg_s: f64,
}

const REQUIRED_SCENARIOS: [RecordingScenario; 9] = [
    RecordingScenario::Stationary,
    RecordingScenario::RollRight,
    RecordingScenario::RollLeft,
    RecordingScenario::PitchFrontDown,
    RecordingScenario::PitchFrontUp,
    RecordingScenario::YawClockwise,
    RecordingScenario::YawCounterclockwise,
    RecordingScenario::LinearForward,
    RecordingScenario::LinearBackward,
];

#[derive(Debug)]
pub enum SelectionError {
    Evaluation(EvaluationError),
    InvalidInput(String),
    Policy(String),
}

impl fmt::Display for SelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evaluation(err) => write!(f, "{err}"),
            Self::InvalidInput(message) | Self::Policy(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SelectionError {}

impl From<EvaluationError> for SelectionError {
    fn from(value: EvaluationError) -> Self {
        Self::Evaluation(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectionConfig {
    pub profile_path: PathBuf,
    pub datasets: Vec<PhysicalDatasetSelectionInput>,
    pub low_pass_tau_ms: Vec<f64>,
    pub complementary_tau_ms: Vec<f64>,
}

impl SelectionConfig {
    pub fn new(
        profile_path: PathBuf,
        datasets: Vec<PhysicalDatasetSelectionInput>,
        low_pass_tau_ms: Vec<f64>,
        complementary_tau_ms: Vec<f64>,
    ) -> Result<Self, SelectionError> {
        let _ = EvaluationConfig::new(low_pass_tau_ms.clone(), complementary_tau_ms.clone())?;
        validate_dataset_inputs(&datasets)?;
        Ok(Self {
            profile_path,
            datasets,
            low_pass_tau_ms,
            complementary_tau_ms,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhysicalDatasetSelectionInput {
    pub scenario: RecordingScenario,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TiltEstimatorPolicyV1 {
    pub version: u8,
    pub candidate: PolicyCandidate,
    pub parameters: BTreeMap<String, f64>,
    pub yaw_available: bool,
    pub source: String,
}

impl TiltEstimatorPolicyV1 {
    pub fn validate(&self) -> Result<(), SelectionError> {
        if self.version != POLICY_VERSION {
            return Err(SelectionError::Policy(format!(
                "unsupported policy version: {}",
                self.version
            )));
        }
        if self.yaw_available {
            return Err(SelectionError::Policy(
                "yawAvailable must remain false".to_owned(),
            ));
        }
        if self.source != POLICY_SOURCE {
            return Err(SelectionError::Policy(format!(
                "policy source must be {POLICY_SOURCE}"
            )));
        }
        match self.candidate {
            PolicyCandidate::GravityNoAdditionalAnchorFilter => {
                if !self.parameters.is_empty() {
                    return Err(SelectionError::Policy(
                        "gravity policy must not contain parameters".to_owned(),
                    ));
                }
            }
            PolicyCandidate::LowPassGravity => {
                validate_single_parameter(&self.parameters, "tauMs")?
            }
            PolicyCandidate::ComplementaryGravityGyro => {
                validate_single_parameter(&self.parameters, "correctionTauMs")?
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyCandidate {
    GravityNoAdditionalAnchorFilter,
    LowPassGravity,
    ComplementaryGravityGyro,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionReport {
    pub selection_report_version: u8,
    pub status: SelectionStatus,
    pub methodology: SelectionMethodology,
    pub inputs: SelectionInputs,
    pub configurations: Vec<SelectionConfigurationReport>,
    pub pareto_front: Vec<String>,
    pub recommendation: Option<SelectionRecommendation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inconclusive: Option<InconclusiveExplanation>,
    pub warnings: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InconclusiveExplanation {
    pub remaining_configurations: Vec<String>,
    pub equivalent_or_conflicting_dimensions: Vec<String>,
    pub minimum_additional_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionStatus {
    Selected,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionMethodology {
    pub gates: Vec<String>,
    pub performance_constraints: Vec<PerformanceConstraint>,
    pub decision_priorities: Vec<String>,
    pub dominance_rule: String,
    pub tolerances: SelectionTolerances,
    pub tolerance_rationale: String,
    pub physical_metrics_interpretation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceConstraint {
    pub name: String,
    pub threshold: f64,
    pub unit: String,
    pub rationale: String,
    pub result: String,
    pub threshold_origin: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionInputs {
    pub synthetic_suite: String,
    pub profile: String,
    pub physical_datasets: Vec<PhysicalDatasetInputReport>,
    pub parameter_grid: SelectionParameterGrid,
    pub mounting_premise: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalDatasetInputReport {
    pub scenario: String,
    pub dataset: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionParameterGrid {
    pub baseline: String,
    pub low_pass_tau_ms: Vec<f64>,
    pub complementary_correction_tau_ms: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionConfigurationReport {
    pub id: String,
    pub candidate: String,
    pub parameters: BTreeMap<String, f64>,
    pub eligible: bool,
    pub rejection_reasons: Vec<String>,
    pub dominated_by: Vec<String>,
    pub dominance_evidence: Vec<String>,
    pub gate_results: Vec<GateResult>,
    pub synthetic_metrics: SelectionSyntheticMetrics,
    pub physical_proxies: Vec<PhysicalScenarioProxyReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateResult {
    pub gate: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionSyntheticMetrics {
    pub angular_error_rmse_deg: f64,
    pub angular_error_p95_deg: f64,
    pub angular_error_max_deg: f64,
    pub roll_rmse_deg: f64,
    pub pitch_rmse_deg: f64,
    pub roll_max_abs_error_deg: f64,
    pub pitch_max_abs_error_deg: f64,
    pub final_drift_deg: f64,
    pub worst_overshoot_deg: SelectionMetric,
    pub worst_settling_time_seconds: SelectionMetric,
    pub worst_abs_sine_lag_seconds: SelectionMetric,
    pub gravity_pulse_angular_rmse_deg: SelectionMetric,
    pub gravity_pulse_angular_max_deg: SelectionMetric,
    pub worst_recovery_time_seconds: SelectionMetric,
    pub gyro_bias_final_drift_deg: f64,
    pub irregular_dt_angular_rmse_deg: f64,
    pub mounting_bias_b2_angular_rmse_deg: f64,
    pub pure_yaw_angular_max_deg: f64,
}

impl Default for SelectionSyntheticMetrics {
    fn default() -> Self {
        Self {
            angular_error_rmse_deg: 0.0,
            angular_error_p95_deg: 0.0,
            angular_error_max_deg: 0.0,
            roll_rmse_deg: 0.0,
            pitch_rmse_deg: 0.0,
            roll_max_abs_error_deg: 0.0,
            pitch_max_abs_error_deg: 0.0,
            final_drift_deg: 0.0,
            worst_overshoot_deg: SelectionMetric::unavailable(
                "deg",
                "not evaluated for this configuration",
            ),
            worst_settling_time_seconds: SelectionMetric::unavailable(
                "s",
                "not evaluated for this configuration",
            ),
            worst_abs_sine_lag_seconds: SelectionMetric::unavailable(
                "s",
                "not evaluated for this configuration",
            ),
            gravity_pulse_angular_rmse_deg: SelectionMetric::unavailable(
                "deg",
                "not evaluated for this configuration",
            ),
            gravity_pulse_angular_max_deg: SelectionMetric::unavailable(
                "deg",
                "not evaluated for this configuration",
            ),
            worst_recovery_time_seconds: SelectionMetric::unavailable(
                "s",
                "not evaluated for this configuration",
            ),
            gyro_bias_final_drift_deg: 0.0,
            irregular_dt_angular_rmse_deg: 0.0,
            mounting_bias_b2_angular_rmse_deg: 0.0,
            pure_yaw_angular_max_deg: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionMetric {
    pub status: SelectionMetricStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMetricStatus {
    Available,
    Failed,
    Unavailable,
}

impl SelectionMetric {
    fn available(unit: &str, value: f64) -> Self {
        debug_assert!(value.is_finite());
        Self {
            status: SelectionMetricStatus::Available,
            value: Some(value),
            unit: unit.to_owned(),
            reason: None,
        }
    }

    fn failed(unit: &str, reason: impl Into<String>) -> Self {
        Self {
            status: SelectionMetricStatus::Failed,
            value: None,
            unit: unit.to_owned(),
            reason: Some(reason.into()),
        }
    }

    fn unavailable(unit: &str, reason: impl Into<String>) -> Self {
        Self {
            status: SelectionMetricStatus::Unavailable,
            value: None,
            unit: unit.to_owned(),
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalScenarioProxyReport {
    pub scenario: String,
    pub ground_truth_available: bool,
    pub angular_dispersion_from_mean_deg: f64,
    pub angular_rate_rms_deg_s: f64,
    pub peak_tilt_from_initial_deg: f64,
    pub final_to_initial_deg: f64,
    pub candidate_divergence_mean_deg: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRecommendation {
    pub policy: TiltEstimatorPolicyV1,
    pub selected_configuration: String,
    pub alternatives_nearby: Vec<String>,
    pub rationale: Vec<String>,
    pub loses_on: Vec<String>,
    pub known_risks: Vec<String>,
    pub review_conditions: Vec<String>,
}

pub fn select_tilt_estimator(config: SelectionConfig) -> Result<SelectionReport, SelectionError> {
    validate_physical_dataset_metadata(&config.datasets)?;

    let evaluation_config = EvaluationConfig::new(
        config.low_pass_tau_ms.clone(),
        config.complementary_tau_ms.clone(),
    )?;
    let synthetic_fixtures = synthetic_suite()?;
    let synthetic = evaluate_synthetic_suite(&evaluation_config)?;

    let mut physical_reports = Vec::new();
    let mut sorted_datasets = config.datasets.clone();
    sorted_datasets.sort_by_key(|item| scenario_order(item.scenario));
    for input in &sorted_datasets {
        let report = evaluate_dataset_file(&input.path, &config.profile_path, &evaluation_config)?;
        physical_reports.push((input.scenario, report));
    }

    let mut configurations = Vec::new();
    for synthetic_candidate in &synthetic.configurations {
        let id = synthetic_candidate.candidate.clone();
        let synthetic_metrics =
            extract_synthetic_metrics(synthetic_candidate, &synthetic_fixtures)?;
        let mut gate_results = gates_for_candidate(synthetic_candidate, &synthetic_metrics);
        let mut rejection_reasons = gate_results
            .iter()
            .filter(|gate| !gate.passed)
            .map(|gate| format!("{}: {}", gate.gate, gate.detail))
            .collect::<Vec<_>>();
        let mut physical_proxies = Vec::new();
        for (scenario, physical_report) in &physical_reports {
            let Some(candidate) = physical_report
                .configurations
                .iter()
                .find(|item| item.candidate == id)
            else {
                return Err(SelectionError::InvalidInput(format!(
                    "candidate {id} missing from physical scenario {}",
                    scenario.as_str()
                )));
            };
            physical_proxies.push(extract_physical_proxy(*scenario, candidate)?);
        }
        if physical_proxies.len() != REQUIRED_SCENARIOS.len() {
            gate_results.push(GateResult {
                gate: "all_physical_scenarios".to_owned(),
                passed: false,
                detail: "not all nine physical scenarios were evaluated".to_owned(),
            });
            rejection_reasons.push("all_physical_scenarios: missing physical scenario".to_owned());
        }
        configurations.push(SelectionConfigurationReport {
            id: id.clone(),
            candidate: policy_candidate_label(&id).to_owned(),
            parameters: synthetic_candidate.parameters.clone(),
            eligible: rejection_reasons.is_empty(),
            rejection_reasons,
            dominated_by: Vec::new(),
            dominance_evidence: Vec::new(),
            gate_results,
            synthetic_metrics,
            physical_proxies,
        });
    }

    mark_dominated(&mut configurations);
    let pareto_front = configurations
        .iter()
        .filter(|item| item.eligible && item.dominated_by.is_empty())
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let recommendation = choose_recommendation(&configurations, &pareto_front)?;
    let status = if recommendation.is_some() {
        SelectionStatus::Selected
    } else {
        SelectionStatus::Inconclusive
    };
    let inconclusive = if recommendation.is_none() {
        Some(inconclusive_explanation(&pareto_front))
    } else {
        None
    };

    Ok(SelectionReport {
        selection_report_version: SELECTION_REPORT_VERSION,
        status,
        methodology: methodology(),
        inputs: SelectionInputs {
            synthetic_suite: "synthetic-suite-v1".to_owned(),
            profile: safe_basename(&config.profile_path),
            physical_datasets: sorted_datasets
                .iter()
                .map(|item| PhysicalDatasetInputReport {
                    scenario: item.scenario.as_str().to_owned(),
                    dataset: safe_basename(&item.path),
                })
                .collect(),
            parameter_grid: SelectionParameterGrid {
                baseline: "gravity_no_additional_anchor_filter".to_owned(),
                low_pass_tau_ms: config.low_pass_tau_ms,
                complementary_correction_tau_ms: config.complementary_tau_ms,
            },
            mounting_premise: vec![
                "same phone as the calibration capture".to_owned(),
                "same mounting convention".to_owned(),
                "physical mount materially preserved across captures".to_owned(),
            ],
        },
        configurations,
        pareto_front,
        recommendation,
        inconclusive,
        warnings: vec![
            "physical metrics are behavioral proxies, not angular accuracy or precision".to_owned(),
            "yaw is unavailable and must remain unavailable in the policy".to_owned(),
            "the selected policy is not applied to the live receiver in B3b".to_owned(),
        ],
        limitations: limitations(),
    })
}

pub fn format_human_selection(report: &SelectionReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Anchor Motion Filter Selection v{}\n",
        report.selection_report_version
    ));
    out.push_str(&format!("status: {:?}\n", report.status));
    out.push_str(&format!(
        "configurationsEvaluated: {}\n",
        report.configurations.len()
    ));
    out.push_str(
        "physical metrics are proxies only; same phone and preserved mounting are assumed\n",
    );
    out.push_str("yawAvailable: false\nliveIntegration: not applied in B3b\n");
    out.push_str("gates:\n");
    for gate in &report.methodology.gates {
        out.push_str(&format!("- {gate}\n"));
    }
    out.push_str("eliminatedConfigurations:\n");
    for item in &report.configurations {
        if !item.eligible || !item.dominated_by.is_empty() {
            out.push_str(&format!(
                "- {} eligible={} dominatedBy={:?} reasons={:?}\n",
                item.id, item.eligible, item.dominated_by, item.rejection_reasons
            ));
        }
    }
    out.push_str(&format!("paretoFront: {:?}\n", report.pareto_front));
    if let Some(recommendation) = &report.recommendation {
        out.push_str(&format!(
            "recommendation: {} {:?}\n",
            recommendation.selected_configuration, recommendation.policy.parameters
        ));
        out.push_str(&format!(
            "alternativesNearby: {:?}\n",
            recommendation.alternatives_nearby
        ));
        out.push_str(&format!("losesOn: {:?}\n", recommendation.loses_on));
    } else {
        out.push_str("recommendation: inconclusive\n");
        if let Some(inconclusive) = &report.inconclusive {
            out.push_str(&format!(
                "remainingConfigurations: {:?}\n",
                inconclusive.remaining_configurations
            ));
            out.push_str(&format!(
                "equivalentOrConflictingDimensions: {:?}\n",
                inconclusive.equivalent_or_conflicting_dimensions
            ));
            out.push_str(&format!(
                "minimumAdditionalEvidence: {:?}\n",
                inconclusive.minimum_additional_evidence
            ));
        }
    }
    for warning in &report.warnings {
        out.push_str(&format!("warning: {warning}\n"));
    }
    out.trim_end().to_owned()
}

pub fn default_selection_config(
    profile_path: PathBuf,
    datasets: Vec<PhysicalDatasetSelectionInput>,
) -> Result<SelectionConfig, SelectionError> {
    SelectionConfig::new(
        profile_path,
        datasets,
        DEFAULT_SELECTION_LOW_PASS_TAU_MS.to_vec(),
        DEFAULT_SELECTION_COMPLEMENTARY_TAU_MS.to_vec(),
    )
}

fn inconclusive_explanation(pareto_front: &[String]) -> InconclusiveExplanation {
    InconclusiveExplanation {
        remaining_configurations: pareto_front.to_vec(),
        equivalent_or_conflicting_dimensions: vec![
            "candidate evidence remained equivalent within declared tolerances or conflicted within the same priority group".to_owned(),
            "no weighted score or post-hoc threshold is invented to force a recommendation".to_owned(),
        ],
        minimum_additional_evidence: vec![
            "repeat or extend the evidence for the first conflicting priority dimension, especially gravity-contamination recovery when RMSE/max trade off against recovery failure".to_owned(),
        ],
    }
}

fn validate_dataset_inputs(inputs: &[PhysicalDatasetSelectionInput]) -> Result<(), SelectionError> {
    let mut seen = BTreeMap::new();
    for input in inputs {
        if !REQUIRED_SCENARIOS.contains(&input.scenario) {
            return Err(SelectionError::InvalidInput(format!(
                "unknown physical scenario: {}",
                input.scenario.as_str()
            )));
        }
        if seen
            .insert(input.scenario.as_str().to_owned(), input.path.clone())
            .is_some()
        {
            return Err(SelectionError::InvalidInput(format!(
                "duplicate physical scenario: {}",
                input.scenario.as_str()
            )));
        }
    }
    let missing = REQUIRED_SCENARIOS
        .iter()
        .filter(|scenario| !seen.contains_key(scenario.as_str()))
        .map(|scenario| scenario.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(SelectionError::InvalidInput(format!(
            "missing physical scenarios: {}",
            missing.join(", ")
        )));
    }
    Ok(())
}

fn validate_physical_dataset_metadata(
    inputs: &[PhysicalDatasetSelectionInput],
) -> Result<(), SelectionError> {
    for input in inputs {
        let dataset = load_validated_dataset_file(&input.path).map_err(EvaluationError::from)?;
        if dataset.metadata.scenario != input.scenario {
            return Err(SelectionError::InvalidInput(format!(
                "physical scenario mismatch for {}: argument label is {}, dataset metadata is {}",
                safe_basename(&input.path),
                input.scenario.as_str(),
                dataset.metadata.scenario.as_str()
            )));
        }
    }
    Ok(())
}

fn extract_synthetic_metrics(
    candidate: &crate::motion_filtering::report::CandidateReport,
    fixtures: &[crate::motion_filtering::synthetic::SyntheticFixture],
) -> Result<SelectionSyntheticMetrics, SelectionError> {
    use crate::motion_filtering::report::CandidateReportMetrics;
    let CandidateReportMetrics::Synthetic(summary) = &candidate.summary else {
        return Err(SelectionError::InvalidInput(
            "synthetic candidate did not contain synthetic metrics".to_owned(),
        ));
    };
    let mut metrics = SelectionSyntheticMetrics {
        angular_error_rmse_deg: required(summary.angular_error_rmse_deg.value(), "angular rmse")?,
        angular_error_p95_deg: required(summary.angular_error_p95_deg.value(), "angular p95")?,
        angular_error_max_deg: required(summary.angular_error_max_deg.value(), "angular max")?,
        roll_rmse_deg: required(summary.roll_rmse_deg.value(), "roll rmse")?,
        pitch_rmse_deg: required(summary.pitch_rmse_deg.value(), "pitch rmse")?,
        roll_max_abs_error_deg: required(summary.roll_max_abs_error_deg.value(), "roll max")?,
        pitch_max_abs_error_deg: required(summary.pitch_max_abs_error_deg.value(), "pitch max")?,
        final_drift_deg: required(summary.final_drift_deg.value(), "final drift")?,
        ..SelectionSyntheticMetrics::default()
    };
    metrics.worst_overshoot_deg = max_fixture_metric(
        candidate,
        fixtures,
        EventMetricKind::Step,
        "deg",
        |m| &m.overshoot_deg,
        |value| value,
    );
    metrics.worst_settling_time_seconds = max_fixture_metric(
        candidate,
        fixtures,
        EventMetricKind::Step,
        "s",
        |m| &m.settling_time_seconds,
        |value| value,
    );
    metrics.worst_abs_sine_lag_seconds = max_fixture_metric(
        candidate,
        fixtures,
        EventMetricKind::Sine,
        "s",
        |m| &m.sine_lag_seconds,
        f64::abs,
    );
    metrics.gravity_pulse_angular_rmse_deg =
        fixture_metric_state(candidate, "gravity_pulse", "deg", |m| {
            &m.angular_error_rmse_deg
        });
    metrics.gravity_pulse_angular_max_deg =
        fixture_metric_state(candidate, "gravity_pulse", "deg", |m| {
            &m.angular_error_max_deg
        });
    metrics.worst_recovery_time_seconds = max_fixture_metric(
        candidate,
        fixtures,
        EventMetricKind::GravityContamination,
        "s",
        |m| &m.recovery_time_seconds,
        |value| value,
    );
    metrics.gyro_bias_final_drift_deg =
        fixture_metric(candidate, "gyro_bias", |m| m.final_drift_deg.value())?;
    metrics.irregular_dt_angular_rmse_deg = fixture_metric(candidate, "irregular_dt", |m| {
        m.angular_error_rmse_deg.value()
    })?;
    metrics.mounting_bias_b2_angular_rmse_deg =
        fixture_metric(candidate, "mounting_bias_b2", |m| {
            m.angular_error_rmse_deg.value()
        })?;
    metrics.pure_yaw_angular_max_deg =
        fixture_metric(candidate, "pure_yaw", |m| m.angular_error_max_deg.value())?;
    ensure_finite_synthetic(&metrics)?;
    Ok(metrics)
}

fn extract_physical_proxy(
    scenario: RecordingScenario,
    candidate: &crate::motion_filtering::report::CandidateReport,
) -> Result<PhysicalScenarioProxyReport, SelectionError> {
    use crate::motion_filtering::report::CandidateReportMetrics;
    let CandidateReportMetrics::Physical(metrics) = &candidate.summary else {
        return Err(SelectionError::InvalidInput(
            "physical candidate did not contain physical proxy metrics".to_owned(),
        ));
    };
    Ok(PhysicalScenarioProxyReport {
        scenario: scenario.as_str().to_owned(),
        ground_truth_available: false,
        angular_dispersion_from_mean_deg: required(
            metrics.angular_dispersion_from_mean_deg.value(),
            "physical dispersion",
        )?,
        angular_rate_rms_deg_s: required(metrics.angular_rate_rms_deg_s.value(), "physical rate")?,
        peak_tilt_from_initial_deg: required(
            metrics.peak_tilt_from_initial_deg.value(),
            "physical peak",
        )?,
        final_to_initial_deg: required(metrics.final_to_initial_deg.value(), "physical final")?,
        candidate_divergence_mean_deg: required(
            metrics.candidate_divergence_mean_deg.value(),
            "physical divergence",
        )?,
    })
}

fn gates_for_candidate(
    candidate: &crate::motion_filtering::report::CandidateReport,
    metrics: &SelectionSyntheticMetrics,
) -> Vec<GateResult> {
    vec![
        gate(
            "finite_outputs",
            all_synthetic_finite(metrics),
            "all selected metrics are finite",
        ),
        gate(
            "mandatory_metrics",
            true,
            "mandatory synthetic and proxy metrics are present",
        ),
        gate(
            "irregular_dt_fixture",
            has_fixture(candidate, "irregular_dt"),
            "irregular dt fixture executed",
        ),
        gate(
            "b2_mounting_fixture",
            has_fixture(candidate, "mounting_bias_b2"),
            "B2 mounting/bias fixture executed",
        ),
        gate(
            "yaw_unavailable",
            true,
            "policy contract keeps yawAvailable=false and evaluation keeps yawCalibrated=false",
        ),
    ]
}

fn mark_dominated(configurations: &mut [SelectionConfigurationReport]) {
    let snapshot = configurations.to_vec();
    for item in configurations {
        if !item.eligible {
            continue;
        }
        let mut dominated_by = Vec::new();
        let mut evidence = Vec::new();
        for other in snapshot
            .iter()
            .filter(|other| other.eligible && other.id != item.id)
        {
            if let Some(dimensions) = dominance_evidence(other, item) {
                dominated_by.push(other.id.clone());
                evidence.push(format!(
                    "{} dominates on {}",
                    other.id,
                    dimensions.join(", ")
                ));
            }
        }
        item.dominated_by = dominated_by;
        item.dominance_evidence = evidence;
    }
}

pub fn dominates(a: &SelectionConfigurationReport, b: &SelectionConfigurationReport) -> bool {
    dominance_evidence(a, b).is_some()
}

fn dominance_evidence(
    a: &SelectionConfigurationReport,
    b: &SelectionConfigurationReport,
) -> Option<Vec<String>> {
    let av = objective_vector(a);
    let bv = objective_vector(b);
    let mut better = Vec::new();
    for (x, y) in av.iter().zip(&bv) {
        match compare_objective(x, y) {
            ObjectiveOrdering::Better => better.push(x.name.to_owned()),
            ObjectiveOrdering::Equivalent => {}
            ObjectiveOrdering::Worse | ObjectiveOrdering::Incomparable => return None,
        }
    }
    if better.is_empty() {
        None
    } else {
        Some(better)
    }
}

fn choose_recommendation(
    configurations: &[SelectionConfigurationReport],
    pareto_front: &[String],
) -> Result<Option<SelectionRecommendation>, SelectionError> {
    let mut candidates = pareto_front
        .iter()
        .filter_map(|id| configurations.iter().find(|item| &item.id == id))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(None);
    }

    apply_priority(&mut candidates, |item| {
        comparable_state(
            &item.synthetic_metrics.worst_abs_sine_lag_seconds,
            TOLERANCES.sine_lag_seconds,
        )
    });
    if !apply_gravity_contamination_priority(&mut candidates) {
        return Ok(None);
    }
    apply_priority(&mut candidates, |item| {
        ComparableMetric::available(
            item.synthetic_metrics.gyro_bias_final_drift_deg,
            TOLERANCES.angular_error_deg,
        )
    });
    apply_priority(&mut candidates, |item| {
        ComparableMetric::available(stationary_dispersion(item), TOLERANCES.angular_proxy_deg)
    });
    apply_simplicity_priority(&mut candidates);

    if candidates.len() != 1 {
        return Ok(None);
    }

    let selected = candidates[0];
    let policy = policy_from_configuration(selected)?;
    policy.validate()?;
    let alternatives_nearby = pareto_front
        .iter()
        .filter(|id| *id != &selected.id)
        .take(3)
        .cloned()
        .collect::<Vec<_>>();
    Ok(Some(SelectionRecommendation {
        policy,
        selected_configuration: selected.id.clone(),
        alternatives_nearby,
        rationale: vec![
            "eligible by all B3b gates".to_owned(),
            "on the Pareto front under comparable synthetic metrics".to_owned(),
            "decision priorities apply tolerances before moving to the next dimension: lag, gravity contamination, gyro drift, rest proxy, then simplicity".to_owned(),
        ],
        loses_on: losing_dimensions(selected, configurations),
        known_risks: vec![
            "Android TYPE_GRAVITY may already include vendor filtering".to_owned(),
            "physical captures provide behavioral proxies only".to_owned(),
            "linear acceleration transients can contaminate gravity-derived tilt".to_owned(),
        ],
        review_conditions: vec![
            "different phone, OS sensor stack or mounting convention".to_owned(),
            "new physical captures show larger acceleration-transient sensitivity".to_owned(),
            "live receiver later introduces gaps, stale samples or interpolation policies".to_owned(),
        ],
    }))
}

fn apply_gravity_contamination_priority(
    candidates: &mut Vec<&SelectionConfigurationReport>,
) -> bool {
    if candidates.len() <= 1 {
        return true;
    }
    let snapshot = candidates.clone();
    let retained = snapshot
        .iter()
        .copied()
        .filter(|item| {
            !snapshot
                .iter()
                .any(|other| other.id != item.id && gravity_contamination_dominates(other, item))
        })
        .collect::<Vec<_>>();
    if retained.len() < candidates.len() {
        *candidates = retained;
        return true;
    }
    gravity_contamination_equivalent(&snapshot)
}

fn gravity_contamination_dominates(
    a: &SelectionConfigurationReport,
    b: &SelectionConfigurationReport,
) -> bool {
    let comparisons = gravity_contamination_comparisons(a, b);
    comparisons.iter().all(|ordering| {
        matches!(
            ordering,
            ObjectiveOrdering::Better | ObjectiveOrdering::Equivalent
        )
    }) && comparisons
        .iter()
        .any(|ordering| matches!(ordering, ObjectiveOrdering::Better))
}

fn gravity_contamination_equivalent(configs: &[&SelectionConfigurationReport]) -> bool {
    configs.iter().enumerate().all(|(index, a)| {
        configs.iter().skip(index + 1).all(|b| {
            gravity_contamination_comparisons(a, b)
                .iter()
                .all(|ordering| matches!(ordering, ObjectiveOrdering::Equivalent))
        })
    })
}

fn gravity_contamination_comparisons(
    a: &SelectionConfigurationReport,
    b: &SelectionConfigurationReport,
) -> [ObjectiveOrdering; 3] {
    [
        compare_values_or_state(
            &a.synthetic_metrics.gravity_pulse_angular_rmse_deg,
            &b.synthetic_metrics.gravity_pulse_angular_rmse_deg,
            TOLERANCES.angular_error_deg,
        ),
        compare_values_or_state(
            &a.synthetic_metrics.gravity_pulse_angular_max_deg,
            &b.synthetic_metrics.gravity_pulse_angular_max_deg,
            TOLERANCES.angular_error_deg,
        ),
        compare_values_or_state(
            &a.synthetic_metrics.worst_recovery_time_seconds,
            &b.synthetic_metrics.worst_recovery_time_seconds,
            TOLERANCES.settling_recovery_seconds,
        ),
    ]
}

fn compare_values_or_state(
    a: &SelectionMetric,
    b: &SelectionMetric,
    tolerance: f64,
) -> ObjectiveOrdering {
    compare_objective(
        &objective_state("group", a, tolerance),
        &objective_state("group", b, tolerance),
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ComparableMetric {
    Available { value: f64, tolerance: f64 },
    Failed,
    Unavailable,
}

impl ComparableMetric {
    fn available(value: f64, tolerance: f64) -> Self {
        Self::Available { value, tolerance }
    }
}

fn comparable_state(metric: &SelectionMetric, tolerance: f64) -> ComparableMetric {
    match metric.status {
        SelectionMetricStatus::Available => metric
            .value
            .map(|value| ComparableMetric::Available { value, tolerance })
            .unwrap_or(ComparableMetric::Failed),
        SelectionMetricStatus::Failed => ComparableMetric::Failed,
        SelectionMetricStatus::Unavailable => ComparableMetric::Unavailable,
    }
}

fn apply_priority(
    candidates: &mut Vec<&SelectionConfigurationReport>,
    metric: impl Fn(&SelectionConfigurationReport) -> ComparableMetric,
) {
    if candidates.len() <= 1 {
        return;
    }
    let metrics = candidates
        .iter()
        .map(|item| metric(item))
        .collect::<Vec<_>>();
    if metrics
        .iter()
        .any(|metric| matches!(metric, ComparableMetric::Unavailable))
    {
        return;
    }
    if metrics
        .iter()
        .any(|metric| matches!(metric, ComparableMetric::Available { .. }))
    {
        let available_values = metrics
            .iter()
            .filter_map(|metric| match metric {
                ComparableMetric::Available { value, .. } => Some(*value),
                _ => None,
            })
            .collect::<Vec<_>>();
        let best = available_values
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let tolerance = metrics
            .iter()
            .find_map(|metric| match metric {
                ComparableMetric::Available { tolerance, .. } => Some(*tolerance),
                _ => None,
            })
            .unwrap_or(0.0);
        let retained = candidates
            .iter()
            .zip(metrics.iter())
            .filter_map(|(item, metric)| match metric {
                ComparableMetric::Available { value, .. } if (*value - best).abs() <= tolerance => {
                    Some(*item)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if !retained.is_empty() && retained.len() < candidates.len() {
            *candidates = retained;
        }
    }
}

fn apply_simplicity_priority(candidates: &mut Vec<&SelectionConfigurationReport>) {
    if candidates.len() <= 1 {
        return;
    }
    let best = candidates
        .iter()
        .map(|item| simplicity_rank(&item.id))
        .min()
        .unwrap_or(usize::MAX);
    candidates.retain(|item| simplicity_rank(&item.id) == best);
}

fn simplicity_rank(id: &str) -> usize {
    if id == "gravity_no_additional_anchor_filter" {
        0
    } else if id.starts_with("low_pass_tau_ms_") {
        1
    } else if id.starts_with("complementary_tau_ms_") {
        2
    } else {
        3
    }
}

fn policy_from_configuration(
    selected: &SelectionConfigurationReport,
) -> Result<TiltEstimatorPolicyV1, SelectionError> {
    let candidate = if selected.id == "gravity_no_additional_anchor_filter" {
        PolicyCandidate::GravityNoAdditionalAnchorFilter
    } else if selected.id.starts_with("low_pass_tau_ms_") {
        PolicyCandidate::LowPassGravity
    } else if selected.id.starts_with("complementary_tau_ms_") {
        PolicyCandidate::ComplementaryGravityGyro
    } else {
        return Err(SelectionError::Policy(format!(
            "unknown selected candidate: {}",
            selected.id
        )));
    };
    Ok(TiltEstimatorPolicyV1 {
        version: POLICY_VERSION,
        candidate,
        parameters: selected.parameters.clone(),
        yaw_available: false,
        source: POLICY_SOURCE.to_owned(),
    })
}

fn losing_dimensions(
    selected: &SelectionConfigurationReport,
    configurations: &[SelectionConfigurationReport],
) -> Vec<String> {
    let mut loses = Vec::new();
    let mut eligible = configurations.iter().filter(|item| item.eligible);
    if eligible.clone().any(|item| {
        item.synthetic_metrics.angular_error_rmse_deg
            < selected.synthetic_metrics.angular_error_rmse_deg - TOLERANCES.angular_error_deg
    }) {
        loses.push("synthetic angular RMSE".to_owned());
    }
    let selected_recovery = comparable_state(
        &selected.synthetic_metrics.worst_recovery_time_seconds,
        TOLERANCES.settling_recovery_seconds,
    );
    if eligible.clone().any(|item| {
        metric_beats(
            comparable_state(
                &item.synthetic_metrics.worst_recovery_time_seconds,
                TOLERANCES.settling_recovery_seconds,
            ),
            selected_recovery,
        )
    }) {
        loses.push("gravity contamination recovery time".to_owned());
    }
    if eligible.any(|item| {
        stationary_dispersion(item) < stationary_dispersion(selected) - TOLERANCES.angular_proxy_deg
    }) {
        loses.push("stationary physical proxy dispersion".to_owned());
    }
    loses
}

fn metric_beats(a: ComparableMetric, b: ComparableMetric) -> bool {
    match (a, b) {
        (
            ComparableMetric::Available {
                value: x,
                tolerance,
            },
            ComparableMetric::Available { value: y, .. },
        ) => x < y - tolerance,
        (ComparableMetric::Available { .. }, ComparableMetric::Failed) => true,
        _ => false,
    }
}

fn stationary_dispersion(item: &SelectionConfigurationReport) -> f64 {
    item.physical_proxies
        .iter()
        .find(|proxy| proxy.scenario == "stationary")
        .map(|proxy| proxy.angular_dispersion_from_mean_deg)
        .unwrap_or(f64::INFINITY)
}

#[derive(Debug, Clone, Copy)]
struct Objective<'a> {
    name: &'a str,
    metric: ObjectiveMetric<'a>,
    tolerance: f64,
}

#[derive(Debug, Clone, Copy)]
enum ObjectiveMetric<'a> {
    Value(f64),
    Stateful(&'a SelectionMetric),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectiveOrdering {
    Better,
    Equivalent,
    Worse,
    Incomparable,
}

fn objective_vector(item: &SelectionConfigurationReport) -> Vec<Objective<'_>> {
    let metrics = &item.synthetic_metrics;
    vec![
        objective_value(
            "synthetic angular RMSE",
            metrics.angular_error_rmse_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_value(
            "synthetic angular p95",
            metrics.angular_error_p95_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_value(
            "synthetic angular max",
            metrics.angular_error_max_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_value(
            "roll RMSE",
            metrics.roll_rmse_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_value(
            "pitch RMSE",
            metrics.pitch_rmse_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_value(
            "final drift",
            metrics.final_drift_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_state(
            "step overshoot",
            &metrics.worst_overshoot_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_state(
            "step settling",
            &metrics.worst_settling_time_seconds,
            TOLERANCES.settling_recovery_seconds,
        ),
        objective_state(
            "sine lag",
            &metrics.worst_abs_sine_lag_seconds,
            TOLERANCES.sine_lag_seconds,
        ),
        objective_state(
            "gravity pulse angular RMSE",
            &metrics.gravity_pulse_angular_rmse_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_state(
            "gravity pulse angular max",
            &metrics.gravity_pulse_angular_max_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_state(
            "gravity pulse recovery",
            &metrics.worst_recovery_time_seconds,
            TOLERANCES.settling_recovery_seconds,
        ),
        objective_value(
            "gyro bias final drift",
            metrics.gyro_bias_final_drift_deg,
            TOLERANCES.angular_error_deg,
        ),
        objective_value(
            "irregular dt angular RMSE",
            metrics.irregular_dt_angular_rmse_deg,
            TOLERANCES.angular_error_deg,
        ),
    ]
}

fn objective_value(name: &'static str, value: f64, tolerance: f64) -> Objective<'static> {
    Objective {
        name,
        metric: ObjectiveMetric::Value(value),
        tolerance,
    }
}

fn objective_state<'a>(
    name: &'static str,
    metric: &'a SelectionMetric,
    tolerance: f64,
) -> Objective<'a> {
    Objective {
        name,
        metric: ObjectiveMetric::Stateful(metric),
        tolerance,
    }
}

fn compare_objective(a: &Objective<'_>, b: &Objective<'_>) -> ObjectiveOrdering {
    match (
        objective_status_and_value(a.metric),
        objective_status_and_value(b.metric),
    ) {
        (
            (SelectionMetricStatus::Available, Some(x)),
            (SelectionMetricStatus::Available, Some(y)),
        ) => compare_values(x, y, a.tolerance),
        ((SelectionMetricStatus::Available, Some(_)), (SelectionMetricStatus::Failed, None)) => {
            ObjectiveOrdering::Better
        }
        ((SelectionMetricStatus::Failed, None), (SelectionMetricStatus::Available, Some(_))) => {
            ObjectiveOrdering::Worse
        }
        ((SelectionMetricStatus::Failed, None), (SelectionMetricStatus::Failed, None)) => {
            ObjectiveOrdering::Equivalent
        }
        (
            (SelectionMetricStatus::Unavailable, None),
            (SelectionMetricStatus::Unavailable, None),
        ) => ObjectiveOrdering::Equivalent,
        ((SelectionMetricStatus::Unavailable, None), _)
        | (_, (SelectionMetricStatus::Unavailable, None)) => ObjectiveOrdering::Incomparable,
        _ => ObjectiveOrdering::Incomparable,
    }
}

fn objective_status_and_value(metric: ObjectiveMetric<'_>) -> (SelectionMetricStatus, Option<f64>) {
    match metric {
        ObjectiveMetric::Value(value) => (SelectionMetricStatus::Available, Some(value)),
        ObjectiveMetric::Stateful(metric) => (metric.status, metric.value),
    }
}

fn compare_values(a: f64, b: f64, tolerance: f64) -> ObjectiveOrdering {
    if (a - b).abs() <= tolerance {
        ObjectiveOrdering::Equivalent
    } else if a < b {
        ObjectiveOrdering::Better
    } else {
        ObjectiveOrdering::Worse
    }
}

fn methodology() -> SelectionMethodology {
    SelectionMethodology {
        gates: vec![
            "no evaluation error, panic or non-finite numeric output".to_owned(),
            "deterministic synthetic and physical evaluation pipeline".to_owned(),
            "required synthetic fixtures are present".to_owned(),
            "metrics applicable to declared fixture events are represented with status".to_owned(),
            "policy validates and keeps yawAvailable=false".to_owned(),
            "evaluation input keeps yawCalibrated=false".to_owned(),
            "configuration and parameters are valid".to_owned(),
            "B2 calibration profile is loaded and the mounting fixture is executed".to_owned(),
            "irregular dt fixture is executed".to_owned(),
        ],
        performance_constraints: Vec::new(),
        decision_priorities: vec![
            "numerical stability and contract compliance".to_owned(),
            "dynamic tracking with low lag".to_owned(),
            "resistance to transient gravity contamination".to_owned(),
            "gyro drift correction".to_owned(),
            "rest stability using stationary physical proxy only".to_owned(),
            "simplicity and predictable cost".to_owned(),
        ],
        dominance_rule:
            "strict Pareto dominance over semantically comparable dimensions with declared physical tolerances; available metrics beat failed metrics, failed metrics are never better than finite values, and no opaque weighted sum is used"
                .to_owned(),
        tolerances: TOLERANCES,
        tolerance_rationale:
            "physical equivalence thresholds, not machine epsilon: 0.001 s for sine lag, 0.01 s for settling/recovery, 0.01 deg for angular errors/proxies, and 0.01 deg/s for angular-rate proxies"
                .to_owned(),
        physical_metrics_interpretation:
            "physical captures use behavioral proxies only, never ground-truth angular accuracy"
                .to_owned(),
    }
}

fn limitations() -> Vec<String> {
    vec![
        "B3b does not apply the policy to the live receiver".to_owned(),
        "yaw remains unavailable".to_owned(),
        "no magnetometer, Kalman filter or adaptive linear-acceleration rejection".to_owned(),
        "physical captures assume materially preserved mounting from the B2 profile".to_owned(),
    ]
}

fn validate_single_parameter(
    parameters: &BTreeMap<String, f64>,
    name: &str,
) -> Result<(), SelectionError> {
    if parameters.len() != 1 || !parameters.contains_key(name) {
        return Err(SelectionError::Policy(format!(
            "policy parameters must contain only {name}"
        )));
    }
    let value = parameters[name];
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(SelectionError::Policy(format!(
            "policy parameter {name} must be positive and finite"
        )))
    }
}

fn required(value: Option<f64>, name: &str) -> Result<f64, SelectionError> {
    let Some(value) = value else {
        return Err(SelectionError::InvalidInput(format!(
            "missing required metric: {name}"
        )));
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err(SelectionError::InvalidInput(format!(
            "non-finite required metric: {name}"
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventMetricKind {
    Step,
    Sine,
    GravityContamination,
}

fn max_fixture_metric(
    candidate: &crate::motion_filtering::report::CandidateReport,
    fixtures: &[crate::motion_filtering::synthetic::SyntheticFixture],
    kind: EventMetricKind,
    unit: &str,
    extract: impl Fn(&crate::motion_filtering::report::TiltGroundTruthMetrics) -> &MetricValue,
    map: impl Fn(f64) -> f64,
) -> SelectionMetric {
    let applicable = fixtures
        .iter()
        .filter(|fixture| fixture_matches_event(fixture, kind))
        .map(|fixture| fixture.name.as_str())
        .collect::<Vec<_>>();
    if applicable.is_empty() {
        return SelectionMetric::unavailable(unit, "no applicable fixture declares this event");
    }

    let mut worst: Option<f64> = None;
    let mut failures = Vec::new();
    for fixture_name in applicable {
        let Some(result) = candidate
            .fixture_results
            .iter()
            .find(|item| item.fixture == fixture_name)
        else {
            failures.push(format!(
                "{fixture_name}: applicable fixture missing from report"
            ));
            continue;
        };
        match extract(&result.metrics) {
            MetricValue::Available { value } => {
                let value = map(*value);
                if value.is_finite() {
                    worst = Some(worst.map_or(value, |current| current.max(value)));
                } else {
                    failures.push(format!("{fixture_name}: metric value is not finite"));
                }
            }
            MetricValue::Unavailable { reason } => {
                failures.push(format!("{fixture_name}: {reason}"));
            }
        }
    }

    if !failures.is_empty() {
        SelectionMetric::failed(unit, failures.join("; "))
    } else if let Some(value) = worst {
        SelectionMetric::available(unit, value)
    } else {
        SelectionMetric::failed(unit, "applicable fixtures produced no available values")
    }
}

fn fixture_matches_event(
    fixture: &crate::motion_filtering::synthetic::SyntheticFixture,
    kind: EventMetricKind,
) -> bool {
    use crate::motion_filtering::synthetic::FixtureEvent;
    matches!(
        (&fixture.event, kind),
        (Some(FixtureEvent::Step { .. }), EventMetricKind::Step)
            | (Some(FixtureEvent::Sine { .. }), EventMetricKind::Sine)
            | (
                Some(FixtureEvent::GravityContamination { .. }),
                EventMetricKind::GravityContamination
            )
    )
}

fn fixture_metric_state(
    candidate: &crate::motion_filtering::report::CandidateReport,
    fixture_name: &str,
    unit: &str,
    extract: impl Fn(&crate::motion_filtering::report::TiltGroundTruthMetrics) -> &MetricValue,
) -> SelectionMetric {
    let Some(fixture) = candidate
        .fixture_results
        .iter()
        .find(|fixture| fixture.fixture == fixture_name)
    else {
        return SelectionMetric::failed(unit, format!("missing fixture: {fixture_name}"));
    };
    match extract(&fixture.metrics) {
        MetricValue::Available { value } if value.is_finite() => {
            SelectionMetric::available(unit, *value)
        }
        MetricValue::Available { .. } => {
            SelectionMetric::failed(unit, format!("{fixture_name}: metric value is not finite"))
        }
        MetricValue::Unavailable { reason } => {
            SelectionMetric::failed(unit, format!("{fixture_name}: {reason}"))
        }
    }
}

fn fixture_metric(
    candidate: &crate::motion_filtering::report::CandidateReport,
    fixture_name: &str,
    extract: impl Fn(&crate::motion_filtering::report::TiltGroundTruthMetrics) -> Option<f64>,
) -> Result<f64, SelectionError> {
    let fixture = candidate
        .fixture_results
        .iter()
        .find(|fixture| fixture.fixture == fixture_name)
        .ok_or_else(|| SelectionError::InvalidInput(format!("missing fixture: {fixture_name}")))?;
    required(extract(&fixture.metrics), fixture_name)
}

fn ensure_finite_synthetic(metrics: &SelectionSyntheticMetrics) -> Result<(), SelectionError> {
    for value in scalar_synthetic_values(metrics) {
        if !value.is_finite() {
            return Err(SelectionError::InvalidInput(
                "synthetic metrics contain NaN or infinity".to_owned(),
            ));
        }
    }
    for metric in stateful_synthetic_metrics(metrics) {
        if matches!(metric.status, SelectionMetricStatus::Available)
            && !metric.value.is_some_and(f64::is_finite)
        {
            return Err(SelectionError::InvalidInput(
                "synthetic metrics contain NaN or infinity".to_owned(),
            ));
        }
    }
    Ok(())
}

fn all_synthetic_finite(metrics: &SelectionSyntheticMetrics) -> bool {
    scalar_synthetic_values(metrics)
        .iter()
        .all(|value| value.is_finite())
        && stateful_synthetic_metrics(metrics).iter().all(|metric| {
            !matches!(metric.status, SelectionMetricStatus::Available)
                || metric.value.is_some_and(f64::is_finite)
        })
}

fn scalar_synthetic_values(metrics: &SelectionSyntheticMetrics) -> [f64; 12] {
    [
        metrics.angular_error_rmse_deg,
        metrics.angular_error_p95_deg,
        metrics.angular_error_max_deg,
        metrics.roll_rmse_deg,
        metrics.pitch_rmse_deg,
        metrics.roll_max_abs_error_deg,
        metrics.pitch_max_abs_error_deg,
        metrics.final_drift_deg,
        metrics.gyro_bias_final_drift_deg,
        metrics.irregular_dt_angular_rmse_deg,
        metrics.mounting_bias_b2_angular_rmse_deg,
        metrics.pure_yaw_angular_max_deg,
    ]
}

fn stateful_synthetic_metrics(metrics: &SelectionSyntheticMetrics) -> [&SelectionMetric; 6] {
    [
        &metrics.worst_overshoot_deg,
        &metrics.worst_settling_time_seconds,
        &metrics.worst_abs_sine_lag_seconds,
        &metrics.gravity_pulse_angular_rmse_deg,
        &metrics.gravity_pulse_angular_max_deg,
        &metrics.worst_recovery_time_seconds,
    ]
}

fn has_fixture(candidate: &crate::motion_filtering::report::CandidateReport, name: &str) -> bool {
    candidate
        .fixture_results
        .iter()
        .any(|fixture| fixture.fixture == name)
}

fn gate(name: &str, passed: bool, detail: &str) -> GateResult {
    GateResult {
        gate: name.to_owned(),
        passed,
        detail: detail.to_owned(),
    }
}

fn scenario_order(scenario: RecordingScenario) -> usize {
    REQUIRED_SCENARIOS
        .iter()
        .position(|item| *item == scenario)
        .unwrap_or(usize::MAX)
}

fn safe_basename(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
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

fn policy_candidate_label(id: &str) -> &str {
    if id == "gravity_no_additional_anchor_filter" {
        "gravity_no_additional_anchor_filter"
    } else if id.starts_with("low_pass_tau_ms_") {
        "low_pass_gravity"
    } else if id.starts_with("complementary_tau_ms_") {
        "complementary_gravity_gyro"
    } else {
        "unknown"
    }
}

#[allow(dead_code)]
fn _b3a_default_grids_still_available() -> (&'static [f64], &'static [f64]) {
    (DEFAULT_LOW_PASS_TAU_MS, DEFAULT_COMPLEMENTARY_TAU_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(id: &str, vector_value: f64) -> SelectionConfigurationReport {
        let mut parameters = BTreeMap::new();
        if let Some(tau) = id.strip_prefix("low_pass_tau_ms_") {
            parameters.insert("tauMs".to_owned(), tau.parse::<f64>().unwrap_or(50.0));
        } else if let Some(tau) = id.strip_prefix("complementary_tau_ms_") {
            parameters.insert(
                "correctionTauMs".to_owned(),
                tau.parse::<f64>().unwrap_or(50.0),
            );
        }
        SelectionConfigurationReport {
            id: id.to_owned(),
            candidate: "test".to_owned(),
            parameters,
            eligible: true,
            rejection_reasons: Vec::new(),
            dominated_by: Vec::new(),
            dominance_evidence: Vec::new(),
            gate_results: Vec::new(),
            synthetic_metrics: SelectionSyntheticMetrics {
                angular_error_rmse_deg: vector_value,
                angular_error_p95_deg: vector_value,
                angular_error_max_deg: vector_value,
                roll_rmse_deg: vector_value,
                pitch_rmse_deg: vector_value,
                roll_max_abs_error_deg: vector_value,
                pitch_max_abs_error_deg: vector_value,
                final_drift_deg: vector_value,
                worst_overshoot_deg: SelectionMetric::available("deg", vector_value),
                worst_settling_time_seconds: SelectionMetric::available("s", vector_value),
                worst_abs_sine_lag_seconds: SelectionMetric::available("s", vector_value),
                gravity_pulse_angular_rmse_deg: SelectionMetric::available("deg", vector_value),
                gravity_pulse_angular_max_deg: SelectionMetric::available("deg", vector_value),
                worst_recovery_time_seconds: SelectionMetric::available("s", vector_value),
                gyro_bias_final_drift_deg: vector_value,
                irregular_dt_angular_rmse_deg: vector_value,
                mounting_bias_b2_angular_rmse_deg: vector_value,
                pure_yaw_angular_max_deg: vector_value,
            },
            physical_proxies: Vec::new(),
        }
    }

    fn front(configs: &[SelectionConfigurationReport]) -> Vec<String> {
        configs.iter().map(|item| item.id.clone()).collect()
    }

    fn selected_id(configs: &[SelectionConfigurationReport]) -> Option<String> {
        choose_recommendation(configs, &front(configs))
            .unwrap()
            .map(|recommendation| recommendation.selected_configuration)
    }

    fn with_lag(mut item: SelectionConfigurationReport, lag: f64) -> SelectionConfigurationReport {
        item.synthetic_metrics.worst_abs_sine_lag_seconds = SelectionMetric::available("s", lag);
        item
    }

    fn with_recovery(
        mut item: SelectionConfigurationReport,
        recovery: SelectionMetric,
    ) -> SelectionConfigurationReport {
        item.synthetic_metrics.worst_recovery_time_seconds = recovery;
        item
    }

    fn with_drift(
        mut item: SelectionConfigurationReport,
        drift: f64,
    ) -> SelectionConfigurationReport {
        item.synthetic_metrics.gyro_bias_final_drift_deg = drift;
        item
    }

    fn with_gravity_pulse(
        mut item: SelectionConfigurationReport,
        rmse: f64,
        max: f64,
    ) -> SelectionConfigurationReport {
        item.synthetic_metrics.gravity_pulse_angular_rmse_deg =
            SelectionMetric::available("deg", rmse);
        item.synthetic_metrics.gravity_pulse_angular_max_deg =
            SelectionMetric::available("deg", max);
        item
    }

    fn with_stationary_proxy(
        mut item: SelectionConfigurationReport,
        dispersion: f64,
    ) -> SelectionConfigurationReport {
        item.physical_proxies = vec![PhysicalScenarioProxyReport {
            scenario: "stationary".to_owned(),
            ground_truth_available: false,
            angular_dispersion_from_mean_deg: dispersion,
            angular_rate_rms_deg_s: 0.0,
            peak_tilt_from_initial_deg: 0.0,
            final_to_initial_deg: 0.0,
            candidate_divergence_mean_deg: 0.0,
        }];
        item
    }

    #[test]
    fn pareto_dominance_detects_dominated_and_incomparable_configs() {
        let better = report("better", 1.0);
        let worse = report("worse", 2.0);
        assert!(dominates(&better, &worse));
        assert!(!dominates(&worse, &better));

        let mut a = report("a", 1.0);
        let mut b = report("b", 1.0);
        a.synthetic_metrics.worst_abs_sine_lag_seconds = SelectionMetric::available("s", 2.0);
        b.synthetic_metrics.angular_error_rmse_deg = 2.0;
        assert!(!dominates(&a, &b));
        assert!(!dominates(&b, &a));
    }

    #[test]
    fn unavailable_recovery_settling_and_lag_are_failed_not_zero() {
        let synthetic = evaluate_synthetic_suite(
            &EvaluationConfig::new(vec![50.0], vec![1500.0, 2000.0]).unwrap(),
        )
        .unwrap();
        let fixtures = synthetic_suite().unwrap();
        let slow = synthetic
            .configurations
            .iter()
            .find(|item| item.candidate == "complementary_tau_ms_2000")
            .unwrap();
        let metrics = extract_synthetic_metrics(slow, &fixtures).unwrap();
        assert_eq!(
            metrics.worst_recovery_time_seconds.status,
            SelectionMetricStatus::Failed
        );
        assert!(metrics.worst_recovery_time_seconds.value.is_none());
        assert!(metrics
            .worst_recovery_time_seconds
            .reason
            .as_deref()
            .unwrap()
            .contains("did not recover"));

        let mut settling = report("settling", 1.0);
        settling.synthetic_metrics.worst_settling_time_seconds =
            SelectionMetric::failed("s", "roll_step_positive: did not settle");
        assert_eq!(
            settling
                .synthetic_metrics
                .worst_settling_time_seconds
                .status,
            SelectionMetricStatus::Failed
        );

        let mut lag = report("lag", 1.0);
        lag.synthetic_metrics.worst_abs_sine_lag_seconds =
            SelectionMetric::failed("s", "roll_sine: estimate sine amplitude is degenerate");
        assert_eq!(
            lag.synthetic_metrics.worst_abs_sine_lag_seconds.status,
            SelectionMetricStatus::Failed
        );
    }

    #[test]
    fn every_family_can_be_recommended_without_prefiltering() {
        let baseline = with_lag(report("gravity_no_additional_anchor_filter", 1.0), 0.001);
        let low = with_lag(report("low_pass_tau_ms_50", 1.0), 0.02);
        let comp = with_lag(report("complementary_tau_ms_50", 1.0), 0.03);
        assert_eq!(
            selected_id(&[baseline.clone(), low.clone(), comp.clone()]).as_deref(),
            Some("gravity_no_additional_anchor_filter")
        );

        let baseline = with_lag(report("gravity_no_additional_anchor_filter", 1.0), 0.03);
        let low = with_lag(report("low_pass_tau_ms_50", 1.0), 0.001);
        let comp = with_lag(report("complementary_tau_ms_50", 1.0), 0.03);
        assert_eq!(
            selected_id(&[baseline, low.clone(), comp]).as_deref(),
            Some("low_pass_tau_ms_50")
        );

        let baseline = with_lag(report("gravity_no_additional_anchor_filter", 1.0), 0.03);
        let low = with_lag(report("low_pass_tau_ms_50", 1.0), 0.03);
        let comp = with_lag(report("complementary_tau_ms_50", 1.0), 0.001);
        assert_eq!(
            selected_id(&[baseline, low, comp]).as_deref(),
            Some("complementary_tau_ms_50")
        );
    }

    #[test]
    fn lag_tolerance_ties_noise_but_not_physical_difference() {
        let a = with_lag(report("complementary_tau_ms_250", 1.0), 4.55e-17);
        let b = with_lag(report("complementary_tau_ms_400", 1.0), 1.41e-15);
        assert_eq!(selected_id(&[a, b]), None);

        let a = with_lag(report("complementary_tau_ms_250", 1.0), 0.0);
        let b = with_lag(report("complementary_tau_ms_400", 1.0), 0.002);
        assert_eq!(
            selected_id(&[a, b]).as_deref(),
            Some("complementary_tau_ms_250")
        );
    }

    #[test]
    fn recovery_priority_precedes_drift() {
        let better_recovery = with_drift(
            with_recovery(
                with_gravity_pulse(report("low_pass_tau_ms_50", 1.0), 0.0, 0.0),
                SelectionMetric::available("s", 0.05),
            ),
            1.0,
        );
        let better_drift = with_drift(
            with_recovery(
                with_gravity_pulse(report("complementary_tau_ms_50", 1.0), 0.0, 0.0),
                SelectionMetric::available("s", 0.20),
            ),
            0.0,
        );
        assert_eq!(
            selected_id(&[better_recovery, better_drift]).as_deref(),
            Some("low_pass_tau_ms_50")
        );
    }

    #[test]
    fn stationary_proxy_and_simplicity_are_late_tie_breakers() {
        let better_proxy = with_stationary_proxy(report("complementary_tau_ms_50", 1.0), 0.0);
        let worse_proxy = with_stationary_proxy(report("low_pass_tau_ms_50", 1.0), 0.02);
        assert_eq!(
            selected_id(&[better_proxy, worse_proxy]).as_deref(),
            Some("complementary_tau_ms_50")
        );

        let baseline =
            with_stationary_proxy(report("gravity_no_additional_anchor_filter", 1.0), 0.0);
        let low = with_stationary_proxy(report("low_pass_tau_ms_50", 1.0), 0.0);
        assert_eq!(
            selected_id(&[baseline, low]).as_deref(),
            Some("gravity_no_additional_anchor_filter")
        );
    }

    #[test]
    fn inconclusive_is_returned_for_empty_or_indistinguishable_candidates() {
        assert_eq!(choose_recommendation(&[], &[]).unwrap(), None);
        let a = report("complementary_tau_ms_250", 1.0);
        let b = report("complementary_tau_ms_400", 1.0);
        assert_eq!(selected_id(&[a, b]), None);
    }

    #[test]
    fn pareto_handles_failed_incomparable_and_ineligible_candidates() {
        let available = report("available", 1.0);
        let mut failed = report("failed", 1.0);
        failed.synthetic_metrics.worst_recovery_time_seconds =
            SelectionMetric::failed("s", "no recovery");
        assert!(dominates(&available, &failed));
        assert!(!dominates(&failed, &available));

        let mut a = report("a", 1.0);
        let mut b = report("b", 1.0);
        a.synthetic_metrics.worst_abs_sine_lag_seconds = SelectionMetric::available("s", 0.02);
        b.synthetic_metrics.gyro_bias_final_drift_deg = 0.02;
        assert!(!dominates(&a, &b));
        assert!(!dominates(&b, &a));

        let mut configs = vec![available.clone(), failed.clone()];
        configs[1].eligible = false;
        mark_dominated(&mut configs);
        let front = configs
            .iter()
            .filter(|item| item.eligible && item.dominated_by.is_empty())
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(front, vec!["available".to_owned()]);
    }

    #[test]
    fn json_preserves_metric_status_reason_and_policy_only_when_selected() {
        let mut item = report("complementary_tau_ms_50", 1.0);
        item.synthetic_metrics.worst_recovery_time_seconds =
            SelectionMetric::failed("s", "gravity_pulse: did not recover");
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("\"status\":\"failed\""));
        assert!(json.contains("did not recover"));
        assert!(!json.contains("NaN"));
        assert!(!json.contains("Infinity"));

        let report = SelectionReport {
            selection_report_version: SELECTION_REPORT_VERSION,
            status: SelectionStatus::Inconclusive,
            methodology: methodology(),
            inputs: SelectionInputs {
                synthetic_suite: "synthetic-suite-v1".to_owned(),
                profile: "profile.json".to_owned(),
                physical_datasets: Vec::new(),
                parameter_grid: SelectionParameterGrid {
                    baseline: "gravity_no_additional_anchor_filter".to_owned(),
                    low_pass_tau_ms: Vec::new(),
                    complementary_correction_tau_ms: Vec::new(),
                },
                mounting_premise: Vec::new(),
            },
            configurations: vec![item],
            pareto_front: vec!["complementary_tau_ms_50".to_owned()],
            recommendation: None,
            inconclusive: Some(inconclusive_explanation(&[
                "complementary_tau_ms_50".to_owned()
            ])),
            warnings: Vec::new(),
            limitations: Vec::new(),
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"recommendation\":null"));
        assert!(format_human_selection(&report).contains("recommendation: inconclusive"));
        assert!(format_human_selection(&report).contains("minimumAdditionalEvidence"));
    }

    #[test]
    fn policy_contract_rejects_invalid_values_and_yaw() {
        let mut parameters = BTreeMap::new();
        parameters.insert("correctionTauMs".to_owned(), 100.0);
        let valid = TiltEstimatorPolicyV1 {
            version: 1,
            candidate: PolicyCandidate::ComplementaryGravityGyro,
            parameters: parameters.clone(),
            yaw_available: false,
            source: POLICY_SOURCE.to_owned(),
        };
        assert!(valid.validate().is_ok());
        assert!(serde_json::to_string(&valid)
            .unwrap()
            .contains("yawAvailable"));

        let mut yaw = valid.clone();
        yaw.yaw_available = true;
        assert!(yaw.validate().is_err());

        let mut invalid = valid;
        invalid.parameters.insert("extra".to_owned(), 1.0);
        assert!(invalid.validate().is_err());
    }
}
