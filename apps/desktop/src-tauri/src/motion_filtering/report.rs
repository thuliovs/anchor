use crate::motion_filtering::metrics::percentile_r7;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationReport {
    pub evaluation_report_version: u8,
    pub input: EvaluationInput,
    pub ground_truth_available: bool,
    pub yaw_calibrated: bool,
    pub sample_count: usize,
    pub observed_duration_us: u64,
    pub dt_statistics: DtStatistics,
    pub calibration: CalibrationInputReport,
    pub configurations: Vec<CandidateReport>,
    pub warnings: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationInput {
    pub kind: InputKind,
    pub identifier: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_identifier: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub synthetic_fixtures: Vec<SyntheticFixtureSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    SyntheticSuite,
    Dataset,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyntheticFixtureSummary {
    pub name: String,
    pub sample_count: usize,
    pub ground_truth_available: bool,
    pub calibration_kind: String,
    pub calibration_profile_version: u8,
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CalibrationInputReport {
    SyntheticSuiteMixed,
    Profile {
        profile: String,
        mounting_convention: String,
        source_dataset: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DtStatistics {
    pub count: usize,
    pub min_seconds: MetricValue,
    pub mean_seconds: MetricValue,
    pub p95_seconds: MetricValue,
    pub max_seconds: MetricValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateReport {
    pub candidate: String,
    pub parameters: BTreeMap<String, f64>,
    pub summary: CandidateReportMetrics,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixture_results: Vec<FixtureCandidateReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureCandidateReport {
    pub fixture: String,
    pub metrics: TiltGroundTruthMetrics,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "metricSet")]
pub enum CandidateReportMetrics {
    Synthetic(TiltGroundTruthMetrics),
    Physical(PhysicalProxyMetrics),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TiltGroundTruthMetrics {
    pub angular_error_rmse_deg: MetricValue,
    pub angular_error_p95_deg: MetricValue,
    pub angular_error_max_deg: MetricValue,
    pub roll_rmse_deg: MetricValue,
    pub pitch_rmse_deg: MetricValue,
    pub roll_max_abs_error_deg: MetricValue,
    pub pitch_max_abs_error_deg: MetricValue,
    pub tilt_magnitude_rms_deg: MetricValue,
    pub tilt_peak_to_peak_deg: MetricValue,
    pub final_drift_deg: MetricValue,
    pub overshoot_deg: MetricValue,
    pub settling_time_seconds: MetricValue,
    pub sine_lag_seconds: MetricValue,
    pub recovery_time_seconds: MetricValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalProxyMetrics {
    pub angular_dispersion_from_mean_deg: MetricValue,
    pub angular_rate_rms_deg_s: MetricValue,
    pub peak_tilt_from_initial_deg: MetricValue,
    pub final_to_initial_deg: MetricValue,
    pub candidate_divergence_mean_deg: MetricValue,
    pub ground_truth_accuracy_metrics: MetricValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum MetricValue {
    Available { value: f64 },
    Unavailable { reason: String },
}
impl MetricValue {
    pub fn available(value: f64) -> Self {
        assert!(value.is_finite());
        Self::Available { value }
    }
    pub fn unavailable(reason: &str) -> Self {
        Self::Unavailable {
            reason: reason.to_owned(),
        }
    }
    pub fn value(&self) -> Option<f64> {
        match self {
            Self::Available { value } => Some(*value),
            Self::Unavailable { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CandidateRun {
    GravityOnly,
    LowPass { tau_ms: f64 },
    Complementary { correction_tau_ms: f64 },
}
impl CandidateRun {
    pub fn candidate_id(self) -> String {
        match self {
            Self::GravityOnly => "gravity_no_additional_anchor_filter".to_owned(),
            Self::LowPass { tau_ms } => format!("low_pass_tau_ms_{}", fmt_tau(tau_ms)),
            Self::Complementary { correction_tau_ms } => {
                format!("complementary_tau_ms_{}", fmt_tau(correction_tau_ms))
            }
        }
    }
    pub fn parameters(self) -> BTreeMap<String, f64> {
        let mut p = BTreeMap::new();
        match self {
            Self::GravityOnly => {}
            Self::LowPass { tau_ms } => {
                p.insert("tauMs".to_owned(), tau_ms);
            }
            Self::Complementary { correction_tau_ms } => {
                p.insert("correctionTauMs".to_owned(), correction_tau_ms);
            }
        }
        p
    }
}
fn fmt_tau(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}").replace('.', "_")
    }
}

pub fn format_human_report(report: &EvaluationReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Anchor Motion Filter Evaluation v{}\n",
        report.evaluation_report_version
    ));
    out.push_str(&format!(
        "input: {:?} {}\n",
        report.input.kind, report.input.identifier
    ));
    if let Some(profile) = &report.input.profile_identifier {
        out.push_str(&format!("profile: {profile}\n"));
    }
    out.push_str(&format!(
        "groundTruthAvailable: {}\nyawCalibrated: {}\nsamples: {}\nobservedDurationUs: {}\n",
        report.ground_truth_available,
        report.yaw_calibrated,
        report.sample_count,
        report.observed_duration_us
    ));
    if !report.ground_truth_available {
        out.push_str("physical results are behavioral proxies, not angular accuracy\nmounting premise: same phone and preserved mounting as calibration capture\n");
    }
    out.push_str("winner: none selected in B3a\n");
    for c in &report.configurations {
        out.push_str(&format!(
            "configuration: {} {:?}\n",
            c.candidate, c.parameters
        ));
        match &c.summary {
            CandidateReportMetrics::Synthetic(m) => out.push_str(&format!("  angularRmseDeg={} angularP95Deg={} rollRmseDeg={} pitchRmseDeg={}\n", mv(&m.angular_error_rmse_deg), mv(&m.angular_error_p95_deg), mv(&m.roll_rmse_deg), mv(&m.pitch_rmse_deg))),
            CandidateReportMetrics::Physical(m) => out.push_str(&format!("  proxyDispersionDeg={} proxyRateRmsDegS={} peakTiltInitialDeg={} finalInitialDeg={} divergenceDeg={}\n", mv(&m.angular_dispersion_from_mean_deg), mv(&m.angular_rate_rms_deg_s), mv(&m.peak_tilt_from_initial_deg), mv(&m.final_to_initial_deg), mv(&m.candidate_divergence_mean_deg))),
        }
    }
    for warning in &report.warnings {
        out.push_str(&format!("warning: {warning}\n"));
    }
    for limitation in &report.limitations {
        out.push_str(&format!("limitation: {limitation}\n"));
    }
    out.trim_end().to_owned()
}
fn mv(value: &MetricValue) -> String {
    match value {
        MetricValue::Available { value } => format!("{value:.6}"),
        MetricValue::Unavailable { reason } => format!("n/a({reason})"),
    }
}

pub fn summarize(values: &[f64]) -> (MetricValue, MetricValue, MetricValue) {
    if values.is_empty() {
        return (
            MetricValue::unavailable("no samples"),
            MetricValue::unavailable("no samples"),
            MetricValue::unavailable("no samples"),
        );
    }
    let rmse = (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt();
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    (
        MetricValue::available(rmse),
        MetricValue::available(percentile_r7(&sorted, 0.95)),
        MetricValue::available(*sorted.last().unwrap()),
    )
}
