use crate::{
    dataset::{
        analyze_dataset_str, load_validated_dataset_str, DatasetAnalysis, DatasetAnalysisError,
        DatasetSampleRecord, RecordingScenario, ValidatedDataset, DATASET_FORMAT_VERSION,
    },
    protocol::{Vector3, PROTOCOL_VERSION},
};
use serde::{Deserialize, Serialize};
use std::{
    fmt, fs,
    fs::OpenOptions,
    io,
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub const CALIBRATION_PROFILE_VERSION: u8 = 1;
pub const CALIBRATION_METHOD: &str = "stationary_level_and_bias_v1";
pub const DEFAULT_CALIBRATION_DIRECTORY: &str = "artifacts/motion-calibrations";
pub const LEVELED_MOUNTING_CONVENTION: &str =
    "leveled_mounting_frame_screen_up_portrait_top_toward_vehicle_front_yaw_unconstrained_v1";
pub const CALIBRATION_QUALITY_CRITERIA: &str =
    "stationary_calibration_v1_initial_diagnostic_thresholds";

pub const MIN_CALIBRATION_SAMPLE_COUNT: usize = 180;
pub const MIN_CALIBRATION_DURATION_US: u64 = 3_000_000;
pub const MAX_RECORDER_DROPPED_SAMPLES: u64 = 0;
pub const MAX_MISSING_SEQUENCE_FRACTION: f64 = 0.01;
pub const MIN_SOURCE_AVERAGE_RATE_HZ: f64 = 45.0;
pub const MAX_SOURCE_AVERAGE_RATE_HZ: f64 = 75.0;
pub const MIN_GRAVITY_MAGNITUDE_MEAN_MPS2: f64 = 9.0;
pub const MAX_GRAVITY_MAGNITUDE_MEAN_MPS2: f64 = 10.5;
pub const MAX_GRAVITY_MAGNITUDE_STDDEV_MPS2: f64 = 0.10;
pub const MAX_LINEAR_ACCELERATION_RMS_MPS2: f64 = 0.35;
pub const MAX_ANGULAR_VELOCITY_RMS_RAD_S: f64 = 0.10;
pub const MAX_TILT_CORRECTION_DEGREES: f64 = 30.0;

const ZERO_VECTOR_NORM_EPSILON: f64 = 1e-12;
const UNIT_QUATERNION_TOLERANCE: f64 = 1e-6;
const CANONICAL_COMPONENT_EPSILON: f64 = 1e-12;
const THRESHOLD_COMPARISON_EPSILON: f64 = 1e-9;
const PROFILE_CONSISTENCY_TOLERANCE: f64 = 1e-6;
const TEMP_FILE_CREATE_ATTEMPTS: u32 = 32;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationReport {
    pub profile: CalibrationProfileV1,
    pub residuals: CalibrationResiduals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationProfileV1 {
    pub calibration_profile_version: u8,
    pub method: String,
    pub created_at_utc: String,
    pub source_dataset: String,
    pub source_started_at_utc: String,
    pub source_dataset_format_version: u8,
    pub source_protocol_version: u8,
    pub source_session_id: String,
    pub source_sample_count: usize,
    pub source_observed_duration_us: u64,
    pub mounting_convention: String,
    pub yaw_calibrated: bool,
    pub device_frame_mean_gravity_mps2: Vector3,
    pub device_frame_linear_acceleration_bias_mps2: Vector3,
    pub device_frame_angular_velocity_bias_rad_s: Vector3,
    pub gravity_magnitude_mean_mps2: f64,
    pub device_to_leveled_quaternion: UnitQuaternion,
    pub tilt_correction_degrees: f64,
    pub quality: CalibrationQualityReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitQuaternion {
    pub w: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationQualityReport {
    pub passed: bool,
    pub criteria: String,
    pub diagnostics: CalibrationDiagnostics,
    pub residuals: CalibrationResiduals,
    pub violations: Vec<CalibrationViolation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationDiagnostics {
    pub sample_count: usize,
    pub session_count: usize,
    pub dataset_complete: bool,
    pub recorder_dropped_samples: u64,
    pub missing_sequence_fraction: f64,
    pub source_average_rate_hz: f64,
    pub gravity_magnitude_mean_mps2: f64,
    pub gravity_magnitude_stddev_mps2: f64,
    pub linear_acceleration_rms_mps2: f64,
    pub angular_velocity_rms_rad_s: f64,
    pub tilt_correction_degrees: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationResiduals {
    pub gravity_calibrated_mean_mps2: Vector3,
    pub gravity_calibrated_magnitude_mean_mps2: f64,
    pub linear_calibrated_mean_mps2: Vector3,
    pub linear_calibrated_rms_mps2: f64,
    pub angular_calibrated_mean_rad_s: Vector3,
    pub angular_calibrated_rms_rad_s: f64,
    pub gravity_angular_error_degrees: ResidualScalarStats,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResidualScalarStats {
    pub mean: f64,
    pub rms: f64,
    pub max: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationViolation {
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
pub enum CalibrationError {
    Dataset(DatasetAnalysisError),
    QualityGateFailed(Box<CalibrationQualityReport>),
    InvalidProfile(String),
    Io(io::Error),
}

impl fmt::Display for CalibrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dataset(err) => write!(f, "{err}"),
            Self::QualityGateFailed(report) => {
                writeln!(f, "dataset failed calibration quality gate v1:")?;
                for violation in &report.violations {
                    writeln!(f, "- {}", violation.message)?;
                }
                Ok(())
            }
            Self::InvalidProfile(message) => write!(f, "invalid calibration profile: {message}"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for CalibrationError {}

impl From<DatasetAnalysisError> for CalibrationError {
    fn from(value: DatasetAnalysisError) -> Self {
        Self::Dataset(value)
    }
}

impl From<io::Error> for CalibrationError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn calibrate_dataset_file(path: &Path) -> Result<CalibrationReport, CalibrationError> {
    calibrate_dataset_file_at(path, SystemTime::now())
}

pub fn calibrate_dataset_file_at(
    path: &Path,
    created_at: SystemTime,
) -> Result<CalibrationReport, CalibrationError> {
    let contents = fs::read_to_string(path)?;
    let dataset = load_validated_dataset_str(&contents)?;
    let analysis = analyze_dataset_str(&contents)?;
    calibrate_from_validated_dataset(
        dataset,
        analysis,
        safe_source_dataset_identifier(path)?,
        created_at,
    )
}

pub fn calibrate_dataset_str(
    contents: &str,
    source_dataset: &str,
) -> Result<CalibrationReport, CalibrationError> {
    calibrate_dataset_str_at(contents, source_dataset, SystemTime::now())
}

pub fn calibrate_dataset_str_at(
    contents: &str,
    source_dataset: &str,
    created_at: SystemTime,
) -> Result<CalibrationReport, CalibrationError> {
    let dataset = load_validated_dataset_str(contents)?;
    let analysis = analyze_dataset_str(contents)?;
    calibrate_from_validated_dataset(
        dataset,
        analysis,
        sanitize_source_dataset_identifier(source_dataset),
        created_at,
    )
}

pub fn load_calibration_profile_file(
    path: &Path,
) -> Result<CalibrationProfileV1, CalibrationError> {
    let contents = fs::read_to_string(path)?;
    load_calibration_profile_str(&contents)
}

pub fn load_calibration_profile_str(
    contents: &str,
) -> Result<CalibrationProfileV1, CalibrationError> {
    let mut profile: CalibrationProfileV1 = serde_json::from_str(contents)
        .map_err(|err| CalibrationError::InvalidProfile(err.to_string()))?;
    validate_profile(&mut profile)?;
    Ok(profile)
}

pub fn write_calibration_profile_file(
    output_path: &Path,
    profile: &CalibrationProfileV1,
) -> Result<(), CalibrationError> {
    let mut profile = profile.clone();
    validate_profile(&mut profile)?;

    let parent = output_path.parent().ok_or_else(|| {
        CalibrationError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "output path has no parent directory: {}",
                output_path.display()
            ),
        ))
    })?;
    fs::create_dir_all(parent)?;
    let rendered = serde_json::to_string_pretty(&profile).map_err(|err| {
        CalibrationError::InvalidProfile(format!("failed to serialize profile: {err}"))
    })?;
    publish_profile_without_overwrite(output_path, format!("{rendered}\n").as_bytes())
}

pub fn default_profile_output_path_for_dataset(dataset_path: &Path) -> PathBuf {
    let stem = dataset_path
        .file_stem()
        .or_else(|| dataset_path.file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("dataset");
    let safe = sanitize_path_component(stem);
    Path::new(DEFAULT_CALIBRATION_DIRECTORY).join(format!("{safe}-calibration-v1.json"))
}

pub fn format_human_report(report: &CalibrationReport, dataset_path: &Path) -> String {
    let profile = &report.profile;
    format!(
        concat!(
            "Anchor Motion Calibration v1\n",
            "dataset: {}\n",
            "profile: {}\n",
            "quality: {}\n",
            "yawCalibrated: {}\n",
            "mountingConvention: {}\n",
            "method: {}\n",
            "createdAtUtc: {}\n",
            "sourceStartedAtUtc: {}\n",
            "tiltCorrectionDegrees: {}\n",
            "deviceFrameMeanGravityMps2: ({}, {}, {})\n",
            "deviceFrameLinearAccelerationBiasMps2: ({}, {}, {})\n",
            "deviceFrameAngularVelocityBiasRadS: ({}, {}, {})\n",
            "deviceToLeveledQuaternion: w={} x={} y={} z={}\n",
            "gravityResidualMeanMps2: ({}, {}, {})\n",
            "gravityResidualMagnitudeMeanMps2: {}\n",
            "linearResidualMeanMps2: ({}, {}, {})\n",
            "linearResidualRmsMps2: {}\n",
            "angularResidualMeanRadS: ({}, {}, {})\n",
            "angularResidualRmsRadS: {}\n",
            "gravityResidualAngle: mean={} rms={} max={} degrees"
        ),
        dataset_path.display(),
        profile.source_dataset,
        if profile.quality.passed {
            "pass"
        } else {
            "fail"
        },
        profile.yaw_calibrated,
        profile.mounting_convention,
        profile.method,
        profile.created_at_utc,
        profile.source_started_at_utc,
        format_f64(profile.tilt_correction_degrees),
        format_f64(profile.device_frame_mean_gravity_mps2.x),
        format_f64(profile.device_frame_mean_gravity_mps2.y),
        format_f64(profile.device_frame_mean_gravity_mps2.z),
        format_f64(profile.device_frame_linear_acceleration_bias_mps2.x),
        format_f64(profile.device_frame_linear_acceleration_bias_mps2.y),
        format_f64(profile.device_frame_linear_acceleration_bias_mps2.z),
        format_f64(profile.device_frame_angular_velocity_bias_rad_s.x),
        format_f64(profile.device_frame_angular_velocity_bias_rad_s.y),
        format_f64(profile.device_frame_angular_velocity_bias_rad_s.z),
        format_f64(profile.device_to_leveled_quaternion.w),
        format_f64(profile.device_to_leveled_quaternion.x),
        format_f64(profile.device_to_leveled_quaternion.y),
        format_f64(profile.device_to_leveled_quaternion.z),
        format_f64(report.residuals.gravity_calibrated_mean_mps2.x),
        format_f64(report.residuals.gravity_calibrated_mean_mps2.y),
        format_f64(report.residuals.gravity_calibrated_mean_mps2.z),
        format_f64(report.residuals.gravity_calibrated_magnitude_mean_mps2),
        format_f64(report.residuals.linear_calibrated_mean_mps2.x),
        format_f64(report.residuals.linear_calibrated_mean_mps2.y),
        format_f64(report.residuals.linear_calibrated_mean_mps2.z),
        format_f64(report.residuals.linear_calibrated_rms_mps2),
        format_f64(report.residuals.angular_calibrated_mean_rad_s.x),
        format_f64(report.residuals.angular_calibrated_mean_rad_s.y),
        format_f64(report.residuals.angular_calibrated_mean_rad_s.z),
        format_f64(report.residuals.angular_calibrated_rms_rad_s),
        format_f64(report.residuals.gravity_angular_error_degrees.mean),
        format_f64(report.residuals.gravity_angular_error_degrees.rms),
        format_f64(report.residuals.gravity_angular_error_degrees.max),
    )
}

pub fn apply_calibration_to_linear(
    profile: &CalibrationProfileV1,
    linear_raw: &Vector3,
) -> Result<Vector3, CalibrationError> {
    validate_finite_vector(linear_raw, "linearRaw")?;
    let mut validated = profile.clone();
    validate_profile(&mut validated)?;
    Ok(validated
        .device_to_leveled_quaternion
        .rotate_vector(&subtract_vectors(
            linear_raw,
            &validated.device_frame_linear_acceleration_bias_mps2,
        )))
}

pub fn apply_calibration_to_angular(
    profile: &CalibrationProfileV1,
    angular_raw: &Vector3,
) -> Result<Vector3, CalibrationError> {
    validate_finite_vector(angular_raw, "angularRaw")?;
    let mut validated = profile.clone();
    validate_profile(&mut validated)?;
    Ok(validated
        .device_to_leveled_quaternion
        .rotate_vector(&subtract_vectors(
            angular_raw,
            &validated.device_frame_angular_velocity_bias_rad_s,
        )))
}

pub fn apply_calibration_to_gravity(
    profile: &CalibrationProfileV1,
    gravity_raw: &Vector3,
) -> Result<Vector3, CalibrationError> {
    validate_finite_vector(gravity_raw, "gravityRaw")?;
    let mut validated = profile.clone();
    validate_profile(&mut validated)?;
    Ok(validated
        .device_to_leveled_quaternion
        .rotate_vector(gravity_raw))
}

fn calibrate_from_validated_dataset(
    dataset: ValidatedDataset,
    analysis: DatasetAnalysis,
    source_dataset: String,
    created_at: SystemTime,
) -> Result<CalibrationReport, CalibrationError> {
    let mean_gravity = mean_vector(dataset.samples.iter().map(|item| &item.sample.gravity_mps2));
    let linear_bias = mean_vector(
        dataset
            .samples
            .iter()
            .map(|item| &item.sample.linear_acceleration_mps2),
    );
    let angular_bias = mean_vector(
        dataset
            .samples
            .iter()
            .map(|item| &item.sample.angular_velocity_rad_s),
    );

    let gravity_direction = normalize_vector(&mean_gravity).ok_or_else(|| {
        CalibrationError::InvalidProfile(
            "mean gravity vector is too small to define a leveled frame".to_owned(),
        )
    })?;
    let leveled_down = Vector3 {
        x: 0.0,
        y: 0.0,
        z: -1.0,
    };
    let quaternion = UnitQuaternion::from_unit_vectors(&gravity_direction, &leveled_down)?;
    let tilt_correction_degrees =
        angle_between_unit_vectors_degrees(&gravity_direction, &leveled_down);

    let residuals = compute_residuals(&dataset.samples, &quaternion, &linear_bias, &angular_bias);
    let diagnostics = build_diagnostics(&analysis, tilt_correction_degrees);
    let violations = collect_quality_violations(&analysis, &diagnostics);
    let quality = CalibrationQualityReport {
        passed: violations.is_empty(),
        criteria: CALIBRATION_QUALITY_CRITERIA.to_owned(),
        diagnostics,
        residuals: residuals.clone(),
        violations,
    };

    let session_id = dataset
        .samples
        .first()
        .map(|item| item.sample.session_id.clone())
        .unwrap_or_default();
    let profile = CalibrationProfileV1 {
        calibration_profile_version: CALIBRATION_PROFILE_VERSION,
        method: CALIBRATION_METHOD.to_owned(),
        created_at_utc: format_rfc3339(created_at)?,
        source_dataset,
        source_started_at_utc: dataset.metadata.started_at_utc.clone(),
        source_dataset_format_version: dataset.metadata.dataset_format_version,
        source_protocol_version: dataset.metadata.protocol_version,
        source_session_id: session_id,
        source_sample_count: dataset.samples.len(),
        source_observed_duration_us: analysis.observed_duration_us,
        mounting_convention: LEVELED_MOUNTING_CONVENTION.to_owned(),
        yaw_calibrated: false,
        device_frame_mean_gravity_mps2: mean_gravity,
        device_frame_linear_acceleration_bias_mps2: linear_bias,
        device_frame_angular_velocity_bias_rad_s: angular_bias,
        gravity_magnitude_mean_mps2: analysis.gravity_magnitude.mean,
        device_to_leveled_quaternion: quaternion,
        tilt_correction_degrees,
        quality,
    };

    if !profile.quality.passed {
        return Err(CalibrationError::QualityGateFailed(Box::new(
            profile.quality.clone(),
        )));
    }

    let mut validated_profile = profile;
    validate_profile(&mut validated_profile)?;
    let report = CalibrationReport {
        profile: validated_profile,
        residuals,
    };

    Ok(report)
}

fn build_diagnostics(
    analysis: &DatasetAnalysis,
    tilt_correction_degrees: f64,
) -> CalibrationDiagnostics {
    let missing_sequence_total: u64 = analysis
        .sessions
        .iter()
        .map(|session| session.missing_sequence_total)
        .sum();
    let denominator = analysis.sample_count as f64 + missing_sequence_total as f64;
    let missing_sequence_fraction = if denominator <= 0.0 {
        0.0
    } else {
        missing_sequence_total as f64 / denominator
    };

    CalibrationDiagnostics {
        sample_count: analysis.sample_count,
        session_count: analysis.session_count,
        dataset_complete: analysis.is_complete,
        recorder_dropped_samples: analysis.recorder_dropped_samples,
        missing_sequence_fraction,
        source_average_rate_hz: analysis.source_average_rate_hz,
        gravity_magnitude_mean_mps2: analysis.gravity_magnitude.mean,
        gravity_magnitude_stddev_mps2: analysis.gravity_magnitude.stddev,
        linear_acceleration_rms_mps2: rms_magnitude_from_stats(
            &analysis.vectors.linear_acceleration_mps2,
        ),
        angular_velocity_rms_rad_s: rms_magnitude_from_stats(
            &analysis.vectors.angular_velocity_rad_s,
        ),
        tilt_correction_degrees,
    }
}

fn collect_quality_violations(
    analysis: &DatasetAnalysis,
    diagnostics: &CalibrationDiagnostics,
) -> Vec<CalibrationViolation> {
    let mut violations = Vec::new();

    if analysis.scenario != RecordingScenario::Stationary {
        violations.push(violation(
            "scenario_not_stationary",
            format!(
                "scenario must be stationary for calibration, got {}",
                analysis.scenario.as_str()
            ),
        ));
    }
    if !diagnostics.dataset_complete {
        violations.push(violation(
            "dataset_incomplete",
            "dataset must be complete with a summary record and completed=true".to_owned(),
        ));
    }
    if diagnostics.session_count != 1 {
        violations.push(violation(
            "session_count",
            format!(
                "dataset must contain exactly one session, got {}",
                diagnostics.session_count
            ),
        ));
    }
    if diagnostics.sample_count < MIN_CALIBRATION_SAMPLE_COUNT {
        violations.push(violation(
            "sample_count",
            format!(
                "sample count must be at least {}, got {}",
                MIN_CALIBRATION_SAMPLE_COUNT, diagnostics.sample_count
            ),
        ));
    }
    if analysis.observed_duration_us < MIN_CALIBRATION_DURATION_US {
        violations.push(violation(
            "observed_duration_us",
            format!(
                "observed duration must be at least {} us, got {} us",
                MIN_CALIBRATION_DURATION_US, analysis.observed_duration_us
            ),
        ));
    }
    if diagnostics.recorder_dropped_samples > MAX_RECORDER_DROPPED_SAMPLES {
        violations.push(violation(
            "recorder_dropped_samples",
            format!(
                "recorderDroppedSamples must be {}, got {}",
                MAX_RECORDER_DROPPED_SAMPLES, diagnostics.recorder_dropped_samples
            ),
        ));
    }
    if gt_limit(
        diagnostics.missing_sequence_fraction,
        MAX_MISSING_SEQUENCE_FRACTION,
    ) {
        violations.push(violation(
            "missing_sequence_fraction",
            format!(
                "missing sequence fraction must be at most {:.2}%, got {:.3}%",
                MAX_MISSING_SEQUENCE_FRACTION * 100.0,
                diagnostics.missing_sequence_fraction * 100.0
            ),
        ));
    }
    if lt_limit(
        diagnostics.source_average_rate_hz,
        MIN_SOURCE_AVERAGE_RATE_HZ,
    ) || gt_limit(
        diagnostics.source_average_rate_hz,
        MAX_SOURCE_AVERAGE_RATE_HZ,
    ) {
        violations.push(violation(
            "source_average_rate_hz",
            format!(
                "source average rate must be between {} and {} Hz, got {} Hz",
                format_f64(MIN_SOURCE_AVERAGE_RATE_HZ),
                format_f64(MAX_SOURCE_AVERAGE_RATE_HZ),
                format_f64(diagnostics.source_average_rate_hz)
            ),
        ));
    }
    if lt_limit(
        diagnostics.gravity_magnitude_mean_mps2,
        MIN_GRAVITY_MAGNITUDE_MEAN_MPS2,
    ) || gt_limit(
        diagnostics.gravity_magnitude_mean_mps2,
        MAX_GRAVITY_MAGNITUDE_MEAN_MPS2,
    ) {
        violations.push(violation(
            "gravity_magnitude_mean_mps2",
            format!(
                "gravity magnitude mean must be between {} and {} m/s^2, got {}",
                format_f64(MIN_GRAVITY_MAGNITUDE_MEAN_MPS2),
                format_f64(MAX_GRAVITY_MAGNITUDE_MEAN_MPS2),
                format_f64(diagnostics.gravity_magnitude_mean_mps2)
            ),
        ));
    }
    if gt_limit(
        diagnostics.gravity_magnitude_stddev_mps2,
        MAX_GRAVITY_MAGNITUDE_STDDEV_MPS2,
    ) {
        violations.push(violation(
            "gravity_magnitude_stddev_mps2",
            format!(
                "gravity magnitude stddev must be at most {} m/s^2, got {}",
                format_f64(MAX_GRAVITY_MAGNITUDE_STDDEV_MPS2),
                format_f64(diagnostics.gravity_magnitude_stddev_mps2)
            ),
        ));
    }
    if gt_limit(
        diagnostics.linear_acceleration_rms_mps2,
        MAX_LINEAR_ACCELERATION_RMS_MPS2,
    ) {
        violations.push(violation(
            "linear_acceleration_rms_mps2",
            format!(
                "linear acceleration RMS must be at most {} m/s^2, got {}",
                format_f64(MAX_LINEAR_ACCELERATION_RMS_MPS2),
                format_f64(diagnostics.linear_acceleration_rms_mps2)
            ),
        ));
    }
    if gt_limit(
        diagnostics.angular_velocity_rms_rad_s,
        MAX_ANGULAR_VELOCITY_RMS_RAD_S,
    ) {
        violations.push(violation(
            "angular_velocity_rms_rad_s",
            format!(
                "angular velocity RMS must be at most {} rad/s, got {}",
                format_f64(MAX_ANGULAR_VELOCITY_RMS_RAD_S),
                format_f64(diagnostics.angular_velocity_rms_rad_s)
            ),
        ));
    }
    if gt_limit(
        diagnostics.tilt_correction_degrees,
        MAX_TILT_CORRECTION_DEGREES,
    ) {
        violations.push(violation(
            "tilt_correction_degrees",
            format!(
                "tilt correction must be at most {} degrees, got {}",
                format_f64(MAX_TILT_CORRECTION_DEGREES),
                format_f64(diagnostics.tilt_correction_degrees)
            ),
        ));
    }

    violations
}

fn compute_residuals(
    samples: &[DatasetSampleRecord],
    quaternion: &UnitQuaternion,
    linear_bias: &Vector3,
    angular_bias: &Vector3,
) -> CalibrationResiduals {
    let mut gravity_sum = zero_vector();
    let mut gravity_magnitude_sum = 0.0;
    let mut linear_sum = zero_vector();
    let mut linear_magnitude_squared_sum = 0.0;
    let mut angular_sum = zero_vector();
    let mut angular_magnitude_squared_sum = 0.0;
    let mut gravity_angle_sum = 0.0;
    let mut gravity_angle_squared_sum = 0.0;
    let mut gravity_angle_max = 0.0_f64;

    for record in samples {
        let gravity_calibrated = quaternion.rotate_vector(&record.sample.gravity_mps2);
        gravity_sum = add_vectors(&gravity_sum, &gravity_calibrated);
        gravity_magnitude_sum += magnitude(&gravity_calibrated);

        let linear_calibrated = quaternion.rotate_vector(&subtract_vectors(
            &record.sample.linear_acceleration_mps2,
            linear_bias,
        ));
        linear_sum = add_vectors(&linear_sum, &linear_calibrated);
        linear_magnitude_squared_sum += squared_magnitude(&linear_calibrated);

        let angular_calibrated = quaternion.rotate_vector(&subtract_vectors(
            &record.sample.angular_velocity_rad_s,
            angular_bias,
        ));
        angular_sum = add_vectors(&angular_sum, &angular_calibrated);
        angular_magnitude_squared_sum += squared_magnitude(&angular_calibrated);

        let gravity_direction = normalize_vector(&gravity_calibrated).unwrap_or(Vector3 {
            x: 0.0,
            y: 0.0,
            z: -1.0,
        });
        let angle = angle_between_unit_vectors_degrees(
            &gravity_direction,
            &Vector3 {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
        );
        gravity_angle_sum += angle;
        gravity_angle_squared_sum += angle * angle;
        gravity_angle_max = gravity_angle_max.max(angle);
    }

    let count = samples.len() as f64;
    CalibrationResiduals {
        gravity_calibrated_mean_mps2: scale_vector(&gravity_sum, 1.0 / count),
        gravity_calibrated_magnitude_mean_mps2: gravity_magnitude_sum / count,
        linear_calibrated_mean_mps2: scale_vector(&linear_sum, 1.0 / count),
        linear_calibrated_rms_mps2: (linear_magnitude_squared_sum / count).sqrt(),
        angular_calibrated_mean_rad_s: scale_vector(&angular_sum, 1.0 / count),
        angular_calibrated_rms_rad_s: (angular_magnitude_squared_sum / count).sqrt(),
        gravity_angular_error_degrees: ResidualScalarStats {
            mean: gravity_angle_sum / count,
            rms: (gravity_angle_squared_sum / count).sqrt(),
            max: gravity_angle_max,
        },
    }
}

fn validate_profile(profile: &mut CalibrationProfileV1) -> Result<(), CalibrationError> {
    if profile.calibration_profile_version != CALIBRATION_PROFILE_VERSION {
        return Err(CalibrationError::InvalidProfile(format!(
            "unsupported calibrationProfileVersion: {}",
            profile.calibration_profile_version
        )));
    }
    if profile.method != CALIBRATION_METHOD {
        return Err(CalibrationError::InvalidProfile(format!(
            "unsupported calibration method: {}",
            profile.method
        )));
    }
    if profile.yaw_calibrated {
        return Err(CalibrationError::InvalidProfile(
            "yawCalibrated must remain false for stationary_level_and_bias_v1".to_owned(),
        ));
    }
    if profile.mounting_convention != LEVELED_MOUNTING_CONVENTION {
        return Err(CalibrationError::InvalidProfile(format!(
            "mountingConvention must be {}",
            LEVELED_MOUNTING_CONVENTION
        )));
    }
    validate_source_dataset_identifier(&profile.source_dataset)?;
    let created_at = OffsetDateTime::parse(&profile.created_at_utc, &Rfc3339).map_err(|err| {
        CalibrationError::InvalidProfile(format!("createdAtUtc must be RFC 3339: {err}"))
    })?;
    let source_started_at = OffsetDateTime::parse(&profile.source_started_at_utc, &Rfc3339)
        .map_err(|err| {
            CalibrationError::InvalidProfile(format!("sourceStartedAtUtc must be RFC 3339: {err}"))
        })?;
    if created_at < source_started_at {
        return Err(CalibrationError::InvalidProfile(
            "createdAtUtc must be greater than or equal to sourceStartedAtUtc".to_owned(),
        ));
    }
    if profile.source_dataset_format_version != DATASET_FORMAT_VERSION {
        return Err(CalibrationError::InvalidProfile(format!(
            "sourceDatasetFormatVersion must be {DATASET_FORMAT_VERSION}, got {}",
            profile.source_dataset_format_version
        )));
    }
    if profile.source_protocol_version != PROTOCOL_VERSION {
        return Err(CalibrationError::InvalidProfile(format!(
            "sourceProtocolVersion must be {PROTOCOL_VERSION}, got {}",
            profile.source_protocol_version
        )));
    }
    validate_session_id(&profile.source_session_id)?;
    if profile.source_sample_count < MIN_CALIBRATION_SAMPLE_COUNT {
        return Err(CalibrationError::InvalidProfile(format!(
            "sourceSampleCount must be at least {MIN_CALIBRATION_SAMPLE_COUNT}, got {}",
            profile.source_sample_count
        )));
    }
    if profile.source_observed_duration_us < MIN_CALIBRATION_DURATION_US {
        return Err(CalibrationError::InvalidProfile(format!(
            "sourceObservedDurationUs must be at least {MIN_CALIBRATION_DURATION_US}, got {}",
            profile.source_observed_duration_us
        )));
    }

    profile.device_to_leveled_quaternion = profile
        .device_to_leveled_quaternion
        .validated_existing_unit_quaternion()?;
    validate_finite_vector(
        &profile.device_frame_mean_gravity_mps2,
        "deviceFrameMeanGravityMps2",
    )?;
    validate_finite_vector(
        &profile.device_frame_linear_acceleration_bias_mps2,
        "deviceFrameLinearAccelerationBiasMps2",
    )?;
    validate_finite_vector(
        &profile.device_frame_angular_velocity_bias_rad_s,
        "deviceFrameAngularVelocityBiasRadS",
    )?;
    validate_finite_f64(
        profile.gravity_magnitude_mean_mps2,
        "gravityMagnitudeMeanMps2",
    )?;
    validate_finite_f64(profile.tilt_correction_degrees, "tiltCorrectionDegrees")?;
    validate_quality(&profile.quality, profile)?;
    validate_profile_gravity_alignment(profile)?;
    validate_profile_internal_consistency(profile)?;
    Ok(())
}

fn validate_quality(
    quality: &CalibrationQualityReport,
    profile: &CalibrationProfileV1,
) -> Result<(), CalibrationError> {
    if quality.criteria != CALIBRATION_QUALITY_CRITERIA {
        return Err(CalibrationError::InvalidProfile(format!(
            "quality.criteria must be {CALIBRATION_QUALITY_CRITERIA}"
        )));
    }
    if !quality.passed {
        return Err(CalibrationError::InvalidProfile(
            "quality.passed must be true".to_owned(),
        ));
    }
    if !quality.violations.is_empty() {
        return Err(CalibrationError::InvalidProfile(
            "quality.violations must be empty for a usable profile".to_owned(),
        ));
    }
    if quality.diagnostics.sample_count != profile.source_sample_count {
        return Err(CalibrationError::InvalidProfile(
            "sourceSampleCount must match quality.diagnostics.sampleCount".to_owned(),
        ));
    }
    if !quality.diagnostics.dataset_complete {
        return Err(CalibrationError::InvalidProfile(
            "quality.diagnostics.datasetComplete must be true".to_owned(),
        ));
    }
    if quality.diagnostics.session_count != 1 {
        return Err(CalibrationError::InvalidProfile(
            "quality.diagnostics.sessionCount must be 1".to_owned(),
        ));
    }
    if quality.diagnostics.recorder_dropped_samples != MAX_RECORDER_DROPPED_SAMPLES {
        return Err(CalibrationError::InvalidProfile(
            "quality.diagnostics.recorderDroppedSamples must be 0".to_owned(),
        ));
    }
    validate_finite_f64(
        quality.diagnostics.missing_sequence_fraction,
        "quality.diagnostics.missingSequenceFraction",
    )?;
    validate_finite_f64(
        quality.diagnostics.source_average_rate_hz,
        "quality.diagnostics.sourceAverageRateHz",
    )?;
    validate_finite_f64(
        quality.diagnostics.gravity_magnitude_mean_mps2,
        "quality.diagnostics.gravityMagnitudeMeanMps2",
    )?;
    validate_finite_f64(
        quality.diagnostics.gravity_magnitude_stddev_mps2,
        "quality.diagnostics.gravityMagnitudeStddevMps2",
    )?;
    validate_finite_f64(
        quality.diagnostics.linear_acceleration_rms_mps2,
        "quality.diagnostics.linearAccelerationRmsMps2",
    )?;
    validate_finite_f64(
        quality.diagnostics.angular_velocity_rms_rad_s,
        "quality.diagnostics.angularVelocityRmsRadS",
    )?;
    validate_finite_f64(
        quality.diagnostics.tilt_correction_degrees,
        "quality.diagnostics.tiltCorrectionDegrees",
    )?;
    validate_diagnostic_limits(&quality.diagnostics)?;
    validate_close(
        profile.tilt_correction_degrees,
        quality.diagnostics.tilt_correction_degrees,
        "tiltCorrectionDegrees must match quality.diagnostics.tiltCorrectionDegrees",
    )?;
    validate_close(
        profile.gravity_magnitude_mean_mps2,
        quality.diagnostics.gravity_magnitude_mean_mps2,
        "gravityMagnitudeMeanMps2 must match quality.diagnostics.gravityMagnitudeMeanMps2",
    )?;
    validate_residuals(&quality.residuals)?;
    validate_gravity_angular_error_stats(&quality.residuals.gravity_angular_error_degrees)?;
    Ok(())
}

fn validate_diagnostic_limits(
    diagnostics: &CalibrationDiagnostics,
) -> Result<(), CalibrationError> {
    if diagnostics.sample_count < MIN_CALIBRATION_SAMPLE_COUNT {
        return Err(CalibrationError::InvalidProfile(
            "quality.diagnostics.sampleCount is below calibration v1 minimum".to_owned(),
        ));
    }
    validate_fraction(
        diagnostics.missing_sequence_fraction,
        "quality.diagnostics.missingSequenceFraction",
    )?;
    validate_non_negative(
        diagnostics.source_average_rate_hz,
        "quality.diagnostics.sourceAverageRateHz",
    )?;
    validate_non_negative(
        diagnostics.gravity_magnitude_mean_mps2,
        "quality.diagnostics.gravityMagnitudeMeanMps2",
    )?;
    validate_non_negative(
        diagnostics.gravity_magnitude_stddev_mps2,
        "quality.diagnostics.gravityMagnitudeStddevMps2",
    )?;
    validate_non_negative(
        diagnostics.linear_acceleration_rms_mps2,
        "quality.diagnostics.linearAccelerationRmsMps2",
    )?;
    validate_non_negative(
        diagnostics.angular_velocity_rms_rad_s,
        "quality.diagnostics.angularVelocityRmsRadS",
    )?;
    validate_non_negative(
        diagnostics.tilt_correction_degrees,
        "quality.diagnostics.tiltCorrectionDegrees",
    )?;
    if gt_limit(
        diagnostics.missing_sequence_fraction,
        MAX_MISSING_SEQUENCE_FRACTION,
    ) || lt_limit(
        diagnostics.source_average_rate_hz,
        MIN_SOURCE_AVERAGE_RATE_HZ,
    ) || gt_limit(
        diagnostics.source_average_rate_hz,
        MAX_SOURCE_AVERAGE_RATE_HZ,
    ) || lt_limit(
        diagnostics.gravity_magnitude_mean_mps2,
        MIN_GRAVITY_MAGNITUDE_MEAN_MPS2,
    ) || gt_limit(
        diagnostics.gravity_magnitude_mean_mps2,
        MAX_GRAVITY_MAGNITUDE_MEAN_MPS2,
    ) || gt_limit(
        diagnostics.gravity_magnitude_stddev_mps2,
        MAX_GRAVITY_MAGNITUDE_STDDEV_MPS2,
    ) || gt_limit(
        diagnostics.linear_acceleration_rms_mps2,
        MAX_LINEAR_ACCELERATION_RMS_MPS2,
    ) || gt_limit(
        diagnostics.angular_velocity_rms_rad_s,
        MAX_ANGULAR_VELOCITY_RMS_RAD_S,
    ) || gt_limit(
        diagnostics.tilt_correction_degrees,
        MAX_TILT_CORRECTION_DEGREES,
    ) {
        return Err(CalibrationError::InvalidProfile(
            "quality.diagnostics must satisfy calibration v1 limits".to_owned(),
        ));
    }
    Ok(())
}

fn validate_residuals(residuals: &CalibrationResiduals) -> Result<(), CalibrationError> {
    validate_finite_vector(
        &residuals.gravity_calibrated_mean_mps2,
        "residuals.gravityCalibratedMeanMps2",
    )?;
    validate_finite_vector(
        &residuals.linear_calibrated_mean_mps2,
        "residuals.linearCalibratedMeanMps2",
    )?;
    validate_finite_vector(
        &residuals.angular_calibrated_mean_rad_s,
        "residuals.angularCalibratedMeanRadS",
    )?;
    validate_finite_f64(
        residuals.gravity_calibrated_magnitude_mean_mps2,
        "residuals.gravityCalibratedMagnitudeMeanMps2",
    )?;
    validate_finite_f64(
        residuals.linear_calibrated_rms_mps2,
        "residuals.linearCalibratedRmsMps2",
    )?;
    validate_finite_f64(
        residuals.angular_calibrated_rms_rad_s,
        "residuals.angularCalibratedRmsRadS",
    )?;
    validate_finite_f64(
        residuals.gravity_angular_error_degrees.mean,
        "residuals.gravityAngularErrorDegrees.mean",
    )?;
    validate_finite_f64(
        residuals.gravity_angular_error_degrees.rms,
        "residuals.gravityAngularErrorDegrees.rms",
    )?;
    validate_finite_f64(
        residuals.gravity_angular_error_degrees.max,
        "residuals.gravityAngularErrorDegrees.max",
    )?;
    validate_non_negative(
        residuals.gravity_calibrated_magnitude_mean_mps2,
        "residuals.gravityCalibratedMagnitudeMeanMps2",
    )?;
    validate_non_negative(
        residuals.linear_calibrated_rms_mps2,
        "residuals.linearCalibratedRmsMps2",
    )?;
    validate_non_negative(
        residuals.angular_calibrated_rms_rad_s,
        "residuals.angularCalibratedRmsRadS",
    )?;
    validate_non_negative(
        residuals.gravity_angular_error_degrees.mean,
        "residuals.gravityAngularErrorDegrees.mean",
    )?;
    validate_non_negative(
        residuals.gravity_angular_error_degrees.rms,
        "residuals.gravityAngularErrorDegrees.rms",
    )?;
    validate_non_negative(
        residuals.gravity_angular_error_degrees.max,
        "residuals.gravityAngularErrorDegrees.max",
    )?;
    Ok(())
}

fn validate_gravity_angular_error_stats(
    stats: &ResidualScalarStats,
) -> Result<(), CalibrationError> {
    for (field_name, value) in [("mean", stats.mean), ("rms", stats.rms), ("max", stats.max)] {
        if value < -PROFILE_CONSISTENCY_TOLERANCE || value > 180.0 + PROFILE_CONSISTENCY_TOLERANCE {
            return Err(CalibrationError::InvalidProfile(format!(
                "residuals.gravityAngularErrorDegrees.{field_name} must be in [0, 180]"
            )));
        }
    }

    if stats.mean > stats.rms + PROFILE_CONSISTENCY_TOLERANCE
        || stats.rms > stats.max + PROFILE_CONSISTENCY_TOLERANCE
    {
        return Err(CalibrationError::InvalidProfile(
            "residuals.gravityAngularErrorDegrees must satisfy mean <= rms <= max".to_owned(),
        ));
    }

    Ok(())
}

fn validate_finite_vector(value: &Vector3, field_name: &str) -> Result<(), CalibrationError> {
    validate_finite_f64(value.x, &format!("{field_name}.x"))?;
    validate_finite_f64(value.y, &format!("{field_name}.y"))?;
    validate_finite_f64(value.z, &format!("{field_name}.z"))?;
    Ok(())
}

fn validate_non_negative(value: f64, field_name: &str) -> Result<(), CalibrationError> {
    if value >= 0.0 {
        Ok(())
    } else {
        Err(CalibrationError::InvalidProfile(format!(
            "{field_name} must be non-negative"
        )))
    }
}

fn validate_fraction(value: f64, field_name: &str) -> Result<(), CalibrationError> {
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(CalibrationError::InvalidProfile(format!(
            "{field_name} must be between 0 and 1"
        )))
    }
}

fn validate_close(actual: f64, expected: f64, message: &str) -> Result<(), CalibrationError> {
    if (actual - expected).abs() <= PROFILE_CONSISTENCY_TOLERANCE {
        Ok(())
    } else {
        Err(CalibrationError::InvalidProfile(message.to_owned()))
    }
}

fn validate_close_rel_abs(
    actual: f64,
    expected: f64,
    message: &str,
) -> Result<(), CalibrationError> {
    let scale = actual.abs().max(expected.abs()).max(1.0);
    let tolerance = PROFILE_CONSISTENCY_TOLERANCE * scale;
    if (actual - expected).abs() <= tolerance {
        Ok(())
    } else {
        Err(CalibrationError::InvalidProfile(message.to_owned()))
    }
}

fn validate_session_id(session_id: &str) -> Result<(), CalibrationError> {
    let len = session_id.chars().count();
    if (1..=64).contains(&len) {
        Ok(())
    } else {
        Err(CalibrationError::InvalidProfile(
            "sourceSessionId must be 1..=64 characters".to_owned(),
        ))
    }
}

fn validate_profile_gravity_alignment(
    profile: &CalibrationProfileV1,
) -> Result<(), CalibrationError> {
    let gravity_direction =
        normalize_vector(&profile.device_frame_mean_gravity_mps2).ok_or_else(|| {
            CalibrationError::InvalidProfile(
                "deviceFrameMeanGravityMps2 must define a non-zero direction".to_owned(),
            )
        })?;
    let leveled_down = Vector3 {
        x: 0.0,
        y: 0.0,
        z: -1.0,
    };
    let expected_tilt = angle_between_unit_vectors_degrees(&gravity_direction, &leveled_down);
    validate_close(
        expected_tilt,
        profile.tilt_correction_degrees,
        "tiltCorrectionDegrees must match deviceFrameMeanGravityMps2 direction",
    )?;

    let calibrated_gravity = profile
        .device_to_leveled_quaternion
        .rotate_vector(&profile.device_frame_mean_gravity_mps2);
    let calibrated_direction = normalize_vector(&calibrated_gravity).ok_or_else(|| {
        CalibrationError::InvalidProfile(
            "deviceToLeveledQuaternion produced degenerate calibrated gravity".to_owned(),
        )
    })?;
    if calibrated_direction.x.abs() > PROFILE_CONSISTENCY_TOLERANCE
        || calibrated_direction.y.abs() > PROFILE_CONSISTENCY_TOLERANCE
        || (calibrated_direction.z + 1.0).abs() > PROFILE_CONSISTENCY_TOLERANCE
    {
        return Err(CalibrationError::InvalidProfile(
            "deviceToLeveledQuaternion must align mean gravity with negative Z".to_owned(),
        ));
    }

    let quaternion_tilt = profile
        .device_to_leveled_quaternion
        .rotation_angle_degrees();
    validate_close(
        quaternion_tilt,
        profile.tilt_correction_degrees,
        "deviceToLeveledQuaternion rotation angle must match tiltCorrectionDegrees",
    )?;
    Ok(())
}

fn validate_finite_f64(value: f64, field_name: &str) -> Result<(), CalibrationError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(CalibrationError::InvalidProfile(format!(
            "{field_name} must be finite"
        )))
    }
}

fn validate_profile_internal_consistency(
    profile: &CalibrationProfileV1,
) -> Result<(), CalibrationError> {
    let residuals = &profile.quality.residuals;
    let diagnostics = &profile.quality.diagnostics;
    let rotated_mean_gravity = profile
        .device_to_leveled_quaternion
        .rotate_vector(&profile.device_frame_mean_gravity_mps2);

    validate_vector_close(
        &rotated_mean_gravity,
        &residuals.gravity_calibrated_mean_mps2,
        "rotated deviceFrameMeanGravityMps2 must match residual gravityCalibratedMeanMps2",
    )?;
    validate_close_rel_abs(
        magnitude(&rotated_mean_gravity),
        magnitude(&profile.device_frame_mean_gravity_mps2),
        "rotated mean gravity norm must match source mean gravity norm",
    )?;
    validate_close_rel_abs(
        residuals.gravity_calibrated_magnitude_mean_mps2,
        profile.gravity_magnitude_mean_mps2,
        "residual gravityCalibratedMagnitudeMeanMps2 must match gravityMagnitudeMeanMps2",
    )?;
    validate_close_rel_abs(
        residuals.gravity_calibrated_magnitude_mean_mps2,
        diagnostics.gravity_magnitude_mean_mps2,
        "residual gravityCalibratedMagnitudeMeanMps2 must match diagnostic gravityMagnitudeMeanMps2",
    )?;

    if magnitude(&profile.device_frame_mean_gravity_mps2)
        > profile.gravity_magnitude_mean_mps2 + PROFILE_CONSISTENCY_TOLERANCE
    {
        return Err(CalibrationError::InvalidProfile(
            "norm(deviceFrameMeanGravityMps2) must not exceed gravityMagnitudeMeanMps2".to_owned(),
        ));
    }

    validate_vector_near_zero(
        &residuals.linear_calibrated_mean_mps2,
        "residual linearCalibratedMeanMps2 must be approximately zero",
    )?;
    validate_vector_near_zero(
        &residuals.angular_calibrated_mean_rad_s,
        "residual angularCalibratedMeanRadS must be approximately zero",
    )?;
    validate_rms_bias_identity(
        residuals.linear_calibrated_rms_mps2,
        &profile.device_frame_linear_acceleration_bias_mps2,
        diagnostics.linear_acceleration_rms_mps2,
        "linear residual RMS and bias must match raw linear RMS",
    )?;
    validate_rms_bias_identity(
        residuals.angular_calibrated_rms_rad_s,
        &profile.device_frame_angular_velocity_bias_rad_s,
        diagnostics.angular_velocity_rms_rad_s,
        "angular residual RMS and bias must match raw angular RMS",
    )?;
    Ok(())
}

fn validate_vector_close(
    actual: &Vector3,
    expected: &Vector3,
    message: &str,
) -> Result<(), CalibrationError> {
    validate_close_rel_abs(actual.x, expected.x, message)?;
    validate_close_rel_abs(actual.y, expected.y, message)?;
    validate_close_rel_abs(actual.z, expected.z, message)?;
    Ok(())
}

fn validate_vector_near_zero(value: &Vector3, message: &str) -> Result<(), CalibrationError> {
    if magnitude(value) <= PROFILE_CONSISTENCY_TOLERANCE {
        Ok(())
    } else {
        Err(CalibrationError::InvalidProfile(message.to_owned()))
    }
}

fn validate_rms_bias_identity(
    residual_rms: f64,
    bias: &Vector3,
    raw_rms: f64,
    message: &str,
) -> Result<(), CalibrationError> {
    validate_close_rel_abs(
        residual_rms * residual_rms + squared_magnitude(bias),
        raw_rms * raw_rms,
        message,
    )
}

fn validate_source_dataset_identifier(source_dataset: &str) -> Result<(), CalibrationError> {
    if source_dataset.is_empty() {
        return Err(CalibrationError::InvalidProfile(
            "sourceDataset must not be empty".to_owned(),
        ));
    }
    let path = Path::new(source_dataset);
    if path.is_absolute() {
        return Err(CalibrationError::InvalidProfile(
            "sourceDataset must not be an absolute path".to_owned(),
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(CalibrationError::InvalidProfile(
                    "sourceDataset must be a safe relative identifier".to_owned(),
                ))
            }
        }
    }
    Ok(())
}

fn safe_source_dataset_identifier(path: &Path) -> Result<String, CalibrationError> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            CalibrationError::InvalidProfile(format!(
                "dataset path must end with a valid UTF-8 file name: {}",
                path.display()
            ))
        })?;
    Ok(sanitize_source_dataset_identifier(file_name))
}

fn sanitize_source_dataset_identifier(source_dataset: &str) -> String {
    let file_name = Path::new(source_dataset)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("dataset.ndjson");
    sanitize_path_component(file_name)
}

fn sanitize_path_component(value: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;

    for ch in value.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            previous_dash = false;
            ch.to_ascii_lowercase()
        } else if ch == '.' {
            previous_dash = false;
            '.'
        } else {
            if previous_dash {
                continue;
            }
            previous_dash = true;
            '-'
        };
        out.push(mapped);
    }

    let trimmed = out.trim_matches('-').trim_matches('.');
    if trimmed.is_empty() {
        "dataset".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn temporary_profile_path_with_attempt(output_path: &Path, attempt: u32) -> PathBuf {
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = output_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("profile.json");
    output_path.with_file_name(format!(
        ".{file_name}.{}.{}.{}.tmp",
        std::process::id(),
        now_nanos,
        attempt
    ))
}

fn publish_profile_without_overwrite(
    output_path: &Path,
    bytes: &[u8],
) -> Result<(), CalibrationError> {
    let mut last_error: Option<io::Error> = None;
    for attempt in 0..TEMP_FILE_CREATE_ATTEMPTS {
        let temp_path = temporary_profile_path_with_attempt(output_path, attempt);
        let temp_file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                last_error = Some(err);
                continue;
            }
            Err(err) => return Err(CalibrationError::Io(err)),
        };

        let result = write_and_publish_temp_profile(temp_file, &temp_path, output_path, bytes);
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        return result.map_err(CalibrationError::Io);
    }
    Err(CalibrationError::Io(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique calibration profile temporary file",
        )
    })))
}

fn write_and_publish_temp_profile(
    mut temp_file: fs::File,
    temp_path: &Path,
    output_path: &Path,
    bytes: &[u8],
) -> io::Result<()> {
    temp_file.write_all(bytes)?;
    temp_file.sync_all()?;
    drop(temp_file);

    match fs::hard_link(temp_path, output_path) {
        Ok(()) => {
            fs::remove_file(temp_path)?;
            Ok(())
        }
        Err(err) => {
            let _ = fs::remove_file(temp_path);
            if err.kind() == io::ErrorKind::AlreadyExists {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "calibration profile already exists: {}",
                        output_path.display()
                    ),
                ))
            } else {
                Err(err)
            }
        }
    }
}

fn violation(code: &str, message: String) -> CalibrationViolation {
    CalibrationViolation {
        code: code.to_owned(),
        message,
    }
}

fn mean_vector<'a>(values: impl Iterator<Item = &'a Vector3>) -> Vector3 {
    let mut sum = zero_vector();
    let mut count = 0.0;
    for value in values {
        sum = add_vectors(&sum, value);
        count += 1.0;
    }
    scale_vector(&sum, 1.0 / count)
}

fn rms_magnitude_from_stats(stats: &crate::dataset::AxisStats3) -> f64 {
    (mean_square(stats.x.mean, stats.x.stddev)
        + mean_square(stats.y.mean, stats.y.stddev)
        + mean_square(stats.z.mean, stats.z.stddev))
    .sqrt()
}

fn mean_square(mean: f64, stddev: f64) -> f64 {
    mean * mean + stddev * stddev
}

fn normalize_vector(value: &Vector3) -> Option<Vector3> {
    let magnitude = magnitude(value);
    if !magnitude.is_finite() || magnitude <= ZERO_VECTOR_NORM_EPSILON {
        return None;
    }
    Some(scale_vector(value, 1.0 / magnitude))
}

fn angle_between_unit_vectors_degrees(first: &Vector3, second: &Vector3) -> f64 {
    dot(first, second).clamp(-1.0, 1.0).acos().to_degrees()
}

fn add_vectors(first: &Vector3, second: &Vector3) -> Vector3 {
    Vector3 {
        x: first.x + second.x,
        y: first.y + second.y,
        z: first.z + second.z,
    }
}

fn subtract_vectors(first: &Vector3, second: &Vector3) -> Vector3 {
    Vector3 {
        x: first.x - second.x,
        y: first.y - second.y,
        z: first.z - second.z,
    }
}

fn scale_vector(value: &Vector3, scalar: f64) -> Vector3 {
    Vector3 {
        x: value.x * scalar,
        y: value.y * scalar,
        z: value.z * scalar,
    }
}

fn zero_vector() -> Vector3 {
    Vector3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    }
}

fn dot(first: &Vector3, second: &Vector3) -> f64 {
    first.x * second.x + first.y * second.y + first.z * second.z
}

fn cross(first: &Vector3, second: &Vector3) -> Vector3 {
    Vector3 {
        x: first.y * second.z - first.z * second.y,
        y: first.z * second.x - first.x * second.z,
        z: first.x * second.y - first.y * second.x,
    }
}

fn magnitude(value: &Vector3) -> f64 {
    squared_magnitude(value).sqrt()
}

fn squared_magnitude(value: &Vector3) -> f64 {
    value.x * value.x + value.y * value.y + value.z * value.z
}

impl UnitQuaternion {
    fn from_unit_vectors(from: &Vector3, to: &Vector3) -> Result<Self, CalibrationError> {
        let dot_value = dot(from, to).clamp(-1.0, 1.0);
        if dot_value >= 1.0 - ZERO_VECTOR_NORM_EPSILON {
            return Ok(Self {
                w: 1.0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            });
        }

        if dot_value <= -1.0 + ZERO_VECTOR_NORM_EPSILON {
            let basis = orthogonal_basis_for(from);
            let axis = normalize_vector(&cross(from, &basis)).ok_or_else(|| {
                CalibrationError::InvalidProfile(
                    "failed to derive a stable 180-degree calibration axis".to_owned(),
                )
            })?;
            return Self {
                w: 0.0,
                x: axis.x,
                y: axis.y,
                z: axis.z,
            }
            .normalize_builder();
        }

        let axis = cross(from, to);
        Self {
            w: 1.0 + dot_value,
            x: axis.x,
            y: axis.y,
            z: axis.z,
        }
        .normalize_builder()
    }

    fn normalize_builder(self) -> Result<Self, CalibrationError> {
        let norm_squared = self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z;
        if !norm_squared.is_finite() || norm_squared <= ZERO_VECTOR_NORM_EPSILON {
            return Err(CalibrationError::InvalidProfile(
                "deviceToLeveledQuaternion must be a non-zero unit quaternion".to_owned(),
            ));
        }
        let inverse_norm = norm_squared.sqrt().recip();
        let canonical = canonicalize_quaternion(Self {
            w: self.w * inverse_norm,
            x: self.x * inverse_norm,
            y: self.y * inverse_norm,
            z: self.z * inverse_norm,
        });
        let normalized_norm = (canonical.w * canonical.w
            + canonical.x * canonical.x
            + canonical.y * canonical.y
            + canonical.z * canonical.z)
            .sqrt();
        if (normalized_norm - 1.0).abs() > UNIT_QUATERNION_TOLERANCE {
            return Err(CalibrationError::InvalidProfile(
                "deviceToLeveledQuaternion must be a unit quaternion".to_owned(),
            ));
        }
        Ok(canonical)
    }

    fn validated_existing_unit_quaternion(self) -> Result<Self, CalibrationError> {
        let norm_squared = self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z;
        if !norm_squared.is_finite() || norm_squared <= ZERO_VECTOR_NORM_EPSILON {
            return Err(CalibrationError::InvalidProfile(
                "deviceToLeveledQuaternion must be a non-zero unit quaternion".to_owned(),
            ));
        }
        let norm = norm_squared.sqrt();
        if (norm - 1.0).abs() > UNIT_QUATERNION_TOLERANCE {
            return Err(CalibrationError::InvalidProfile(
                "deviceToLeveledQuaternion must be a unit quaternion".to_owned(),
            ));
        }
        Ok(canonicalize_quaternion(Self {
            w: self.w / norm,
            x: self.x / norm,
            y: self.y / norm,
            z: self.z / norm,
        }))
    }

    fn rotate_vector(&self, value: &Vector3) -> Vector3 {
        let qv = Vector3 {
            x: self.x,
            y: self.y,
            z: self.z,
        };
        let twice_cross = scale_vector(&cross(&qv, value), 2.0);
        add_vectors(
            value,
            &add_vectors(
                &scale_vector(&twice_cross, self.w),
                &cross(&qv, &twice_cross),
            ),
        )
    }

    fn rotation_angle_degrees(&self) -> f64 {
        let w = self.w.clamp(-1.0, 1.0).abs();
        2.0 * w.acos().to_degrees()
    }
}

fn format_rfc3339(now: SystemTime) -> Result<String, CalibrationError> {
    let utc = OffsetDateTime::from(now);
    utc.format(&Rfc3339).map_err(|err| {
        CalibrationError::Io(io::Error::other(format!(
            "failed to format UTC time: {err}"
        )))
    })
}

fn orthogonal_basis_for(vector: &Vector3) -> Vector3 {
    let abs_x = vector.x.abs();
    let abs_y = vector.y.abs();
    let abs_z = vector.z.abs();
    if abs_x <= abs_y && abs_x <= abs_z {
        Vector3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        }
    } else if abs_y <= abs_z {
        Vector3 {
            x: 0.0,
            y: 1.0,
            z: 0.0,
        }
    } else {
        Vector3 {
            x: 0.0,
            y: 0.0,
            z: 1.0,
        }
    }
}

fn canonicalize_quaternion(quaternion: UnitQuaternion) -> UnitQuaternion {
    if quaternion.w < -CANONICAL_COMPONENT_EPSILON {
        return negate_quaternion(quaternion);
    }
    if quaternion.w > CANONICAL_COMPONENT_EPSILON {
        return quaternion;
    }

    for component in [quaternion.x, quaternion.y, quaternion.z] {
        if component < -CANONICAL_COMPONENT_EPSILON {
            return negate_quaternion(quaternion);
        }
        if component > CANONICAL_COMPONENT_EPSILON {
            return quaternion;
        }
    }
    quaternion
}

fn negate_quaternion(quaternion: UnitQuaternion) -> UnitQuaternion {
    UnitQuaternion {
        w: -quaternion.w,
        x: -quaternion.x,
        y: -quaternion.y,
        z: -quaternion.z,
    }
}

fn format_f64(value: f64) -> String {
    format!("{value:.6}")
}

fn gt_limit(value: f64, limit: f64) -> bool {
    value - limit > THRESHOLD_COMPARISON_EPSILON
}

fn lt_limit(value: f64, limit: f64) -> bool {
    limit - value > THRESHOLD_COMPARISON_EPSILON
}
