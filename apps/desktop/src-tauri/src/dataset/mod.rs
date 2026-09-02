use crate::{
    protocol::{MotionSampleV1, PROTOCOL_VERSION},
    receiver::{AcceptedSampleEvent, AcceptedSampleSink, EXPECTED_SAMPLE_RATE_HZ},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Instant, SystemTime},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::{
    fs::{create_dir_all, OpenOptions},
    io::AsyncWriteExt,
    sync::{mpsc, oneshot, watch, Mutex, Notify},
    task::JoinHandle,
};

pub const DATASET_FORMAT_VERSION: u8 = 1;
pub const DEFAULT_DATASET_DIRECTORY: &str = "artifacts/motion-datasets";
pub const DEFAULT_MOUNTING_CONVENTION: &str = "flat_screen_up_portrait_top_toward_vehicle_front";
pub const DEFAULT_RECORDER_QUEUE_CAPACITY: usize = 256;
pub const MAX_ANALYZABLE_RECEIVED_ELAPSED_US: u64 = i64::MAX as u64;

const FLUSH_EVERY_SAMPLES: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingScenario {
    Stationary,
    RollRight,
    RollLeft,
    PitchFrontDown,
    PitchFrontUp,
    YawClockwise,
    YawCounterclockwise,
    LinearForward,
    LinearBackward,
}

impl RecordingScenario {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stationary => "stationary",
            Self::RollRight => "roll_right",
            Self::RollLeft => "roll_left",
            Self::PitchFrontDown => "pitch_front_down",
            Self::PitchFrontUp => "pitch_front_up",
            Self::YawClockwise => "yaw_clockwise",
            Self::YawCounterclockwise => "yaw_counterclockwise",
            Self::LinearForward => "linear_forward",
            Self::LinearBackward => "linear_backward",
        }
    }
}

impl fmt::Display for RecordingScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for RecordingScenario {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "stationary" => Ok(Self::Stationary),
            "roll_right" => Ok(Self::RollRight),
            "roll_left" => Ok(Self::RollLeft),
            "pitch_front_down" => Ok(Self::PitchFrontDown),
            "pitch_front_up" => Ok(Self::PitchFrontUp),
            "yaw_clockwise" => Ok(Self::YawClockwise),
            "yaw_counterclockwise" => Ok(Self::YawCounterclockwise),
            "linear_forward" => Ok(Self::LinearForward),
            "linear_backward" => Ok(Self::LinearBackward),
            _ => Err(
                "scenario must be one of: stationary, roll_right, roll_left, pitch_front_down, pitch_front_up, yaw_clockwise, yaw_counterclockwise, linear_forward, linear_backward".to_owned()
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DatasetRecorderConfig {
    pub output_path: PathBuf,
    pub scenario: RecordingScenario,
    pub notes: Option<String>,
    pub queue_capacity: usize,
    pub flow_control: Option<DatasetRecorderFlowControl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetMetadataRecord {
    pub record_type: DatasetRecordType,
    pub dataset_format_version: u8,
    pub protocol_version: u8,
    pub scenario: RecordingScenario,
    pub started_at_utc: String,
    pub expected_sample_rate_hz: u16,
    pub mounting_convention: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetSampleRecord {
    pub record_type: DatasetRecordType,
    pub received_elapsed_us: u64,
    pub sample: MotionSampleV1,
    #[serde(skip)]
    pub line_number: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetSummaryRecord {
    pub record_type: DatasetRecordType,
    pub completed: bool,
    pub duration_us: u64,
    pub received_accepted_samples: u64,
    pub written_samples: u64,
    pub recorder_dropped_samples: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetRecordType {
    Metadata,
    Sample,
    Summary,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetAnalysis {
    pub dataset_format_version: u8,
    pub protocol_version: u8,
    pub scenario: RecordingScenario,
    pub mounting_convention: String,
    pub is_complete: bool,
    pub sample_count: usize,
    pub session_count: usize,
    pub observed_duration_us: u64,
    pub source_average_rate_hz: f64,
    pub receive_average_rate_hz: f64,
    pub source_interval_us: IntervalStats,
    pub receive_interval_us: IntervalStats,
    pub relative_delay_variation_us: SignedIntervalStats,
    pub recorder_dropped_samples: u64,
    pub sessions: Vec<SessionAnalysis>,
    pub vectors: VectorFieldStats,
    pub gravity_magnitude: ScalarStats,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAnalysis {
    pub session_id: String,
    pub sample_count: usize,
    pub first_sequence: u32,
    pub last_sequence: u32,
    pub gap_count: usize,
    pub missing_sequence_total: u64,
    pub gaps: Vec<SequenceGap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceGap {
    pub after_sequence: u32,
    pub before_sequence: u32,
    pub missing_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntervalStats {
    pub min: Option<u64>,
    pub mean: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    pub max: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedIntervalStats {
    pub min: Option<i64>,
    pub mean: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    pub max: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorFieldStats {
    pub linear_acceleration_mps2: AxisStats3,
    pub gravity_mps2: AxisStats3,
    pub angular_velocity_rad_s: AxisStats3,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AxisStats3 {
    pub x: ScalarStats,
    pub y: ScalarStats,
    pub z: ScalarStats,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalarStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub stddev: f64,
}

#[derive(Debug)]
pub enum DatasetAnalysisError {
    Io(io::Error),
    MissingMetadata,
    InvalidMetadata {
        line: usize,
        message: String,
    },
    UnsupportedDatasetVersion(u8),
    InvalidDatasetStructure {
        line: usize,
        message: String,
    },
    InvalidJsonLine {
        line: usize,
        message: String,
    },
    InvalidReceivedElapsedUs {
        line: usize,
    },
    ReceivedElapsedUsOutOfRange {
        line: usize,
        value: u64,
        max_supported: u64,
    },
    InvalidMotionSample {
        line: usize,
        message: String,
    },
    ReceivedElapsedRegression {
        line: usize,
        previous_line: usize,
        previous_received_elapsed_us: u64,
        current_received_elapsed_us: u64,
    },
    SessionElapsedRegression {
        line: usize,
        previous_line: usize,
        session_id: String,
        previous_session_elapsed_us: u64,
        current_session_elapsed_us: u64,
    },
    InvalidSummary {
        line: usize,
        message: String,
    },
    NumericOverflow {
        line: usize,
        message: String,
    },
    NoSamples,
}

impl fmt::Display for DatasetAnalysisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::MissingMetadata => {
                f.write_str("dataset metadata record is missing or is not the first line")
            }
            Self::InvalidMetadata { line, message } => {
                write!(f, "invalid metadata at line {line}: {message}")
            }
            Self::UnsupportedDatasetVersion(version) => {
                write!(f, "unsupported datasetFormatVersion: {version}")
            }
            Self::InvalidDatasetStructure { line, message } => {
                write!(f, "invalid dataset structure at line {line}: {message}")
            }
            Self::InvalidJsonLine { line, message } => {
                write!(f, "invalid JSON at line {line}: {message}")
            }
            Self::InvalidReceivedElapsedUs { line } => {
                write!(f, "invalid receivedElapsedUs at line {line}")
            }
            Self::ReceivedElapsedUsOutOfRange {
                line,
                value,
                max_supported,
            } => write!(
                f,
                "receivedElapsedUs at line {line} exceeds supported analysis range: value={value}, maxSupported={max_supported}"
            ),
            Self::InvalidMotionSample { line, message } => {
                write!(f, "invalid MotionSampleV1 at line {line}: {message}")
            }
            Self::ReceivedElapsedRegression {
                line,
                previous_line,
                previous_received_elapsed_us,
                current_received_elapsed_us,
            } => write!(
                f,
                "receivedElapsedUs regressed at line {line}: previous line {previous_line} had {previous_received_elapsed_us}, current value is {current_received_elapsed_us}"
            ),
            Self::SessionElapsedRegression {
                line,
                previous_line,
                session_id,
                previous_session_elapsed_us,
                current_session_elapsed_us,
            } => write!(
                f,
                "sessionElapsedUs regressed for session {session_id} at line {line}: previous line {previous_line} had {previous_session_elapsed_us}, current value is {current_session_elapsed_us}"
            ),
            Self::InvalidSummary { line, message } => {
                write!(f, "invalid summary at line {line}: {message}")
            }
            Self::NumericOverflow { line, message } => {
                write!(f, "numeric overflow while analyzing line {line}: {message}")
            }
            Self::NoSamples => f.write_str("dataset does not contain any valid sample records"),
        }
    }
}

impl std::error::Error for DatasetAnalysisError {}

impl From<io::Error> for DatasetAnalysisError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub struct DatasetRecorderHandle {
    output_path: PathBuf,
    ingress: Arc<dyn AcceptedSampleSink>,
    counters: Arc<RecorderCounters>,
    shutdown_tx: Option<oneshot::Sender<bool>>,
    join_handle: JoinHandle<io::Result<DatasetSummaryRecord>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetRecorderSnapshot {
    pub received_accepted_samples: u64,
    pub written_samples: u64,
    pub recorder_dropped_samples: u64,
}

#[derive(Debug, Default)]
struct RecorderCounters {
    received_accepted_samples: AtomicU64,
    written_samples: AtomicU64,
    recorder_dropped_samples: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct DatasetRecorderFlowControl {
    release_tx: watch::Sender<bool>,
    release_rx: Arc<Mutex<watch::Receiver<bool>>>,
    writer_blocked: Arc<AtomicBool>,
    blocked_notify: Arc<Notify>,
}

impl DatasetRecorderFlowControl {
    pub fn new_blocked() -> Self {
        let (release_tx, release_rx) = watch::channel(false);
        Self {
            release_tx,
            release_rx: Arc::new(Mutex::new(release_rx)),
            writer_blocked: Arc::new(AtomicBool::new(false)),
            blocked_notify: Arc::new(Notify::new()),
        }
    }

    pub async fn wait_until_writer_blocked(&self) {
        loop {
            if self.writer_blocked.load(Ordering::SeqCst) {
                return;
            }
            self.blocked_notify.notified().await;
        }
    }

    pub fn release(&self) {
        let _ = self.release_tx.send(true);
    }

    async fn wait_until_released_for_sample_write(&self) {
        let mut release_rx = self.release_rx.lock().await;
        if *release_rx.borrow() {
            return;
        }

        self.writer_blocked.store(true, Ordering::SeqCst);
        self.blocked_notify.notify_waiters();
        while release_rx.changed().await.is_ok() {
            if *release_rx.borrow_and_update() {
                break;
            }
        }
        self.writer_blocked.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct RecorderIngress {
    tx: mpsc::Sender<AcceptedSampleEvent>,
    counters: Arc<RecorderCounters>,
}

impl AcceptedSampleSink for RecorderIngress {
    fn try_publish(&self, event: AcceptedSampleEvent) {
        self.counters
            .received_accepted_samples
            .fetch_add(1, Ordering::Relaxed);

        if self.tx.try_send(event).is_err() {
            self.counters
                .recorder_dropped_samples
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl DatasetRecorderHandle {
    pub fn ingress(&self) -> Arc<dyn AcceptedSampleSink> {
        self.ingress.clone()
    }

    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    pub fn snapshot(&self) -> DatasetRecorderSnapshot {
        DatasetRecorderSnapshot {
            received_accepted_samples: self
                .counters
                .received_accepted_samples
                .load(Ordering::Relaxed),
            written_samples: self.counters.written_samples.load(Ordering::Relaxed),
            recorder_dropped_samples: self
                .counters
                .recorder_dropped_samples
                .load(Ordering::Relaxed),
        }
    }

    pub async fn shutdown(mut self, completed: bool) -> io::Result<DatasetSummaryRecord> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(completed);
        }

        match self.join_handle.await {
            Ok(result) => result,
            Err(err) => Err(io::Error::other(format!(
                "dataset recorder task join failure: {err}"
            ))),
        }
    }
}

pub async fn start_dataset_recorder(
    config: DatasetRecorderConfig,
) -> io::Result<DatasetRecorderHandle> {
    if config.queue_capacity == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dataset recorder queue capacity must be greater than zero",
        ));
    }

    let started_at = Instant::now();
    let started_at_utc = format_started_at_utc(SystemTime::now())?;
    let metadata = DatasetMetadataRecord {
        record_type: DatasetRecordType::Metadata,
        dataset_format_version: DATASET_FORMAT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        scenario: config.scenario,
        started_at_utc,
        expected_sample_rate_hz: EXPECTED_SAMPLE_RATE_HZ,
        mounting_convention: DEFAULT_MOUNTING_CONVENTION.to_owned(),
        notes: config.notes,
    };

    if let Some(parent) = config.output_path.parent() {
        create_dir_all(parent).await?;
    }

    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&config.output_path)
        .await?;
    write_record_line(&mut file, &metadata).await?;
    file.flush().await?;

    let counters = Arc::new(RecorderCounters::default());
    let (tx, mut rx) = mpsc::channel(config.queue_capacity);
    let ingress: Arc<dyn AcceptedSampleSink> = Arc::new(RecorderIngress {
        tx,
        counters: counters.clone(),
    });
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<bool>();
    let counters_for_task = counters.clone();
    let flow_control = config.flow_control.clone();

    let join_handle = tokio::spawn(async move {
        let mut completed = false;

        let result = async {
            loop {
                tokio::select! {
                    received = &mut shutdown_rx => {
                        completed = received.unwrap_or(false);
                        while let Ok(event) = rx.try_recv() {
                            write_sample_event(&mut file, &counters_for_task, started_at, event, flow_control.as_ref()).await?;
                        }
                        break;
                    }
                    maybe_event = rx.recv() => {
                        match maybe_event {
                            Some(event) => {
                                write_sample_event(&mut file, &counters_for_task, started_at, event, flow_control.as_ref()).await?;
                                let written = counters_for_task.written_samples.load(Ordering::Relaxed);
                                if written % FLUSH_EVERY_SAMPLES == 0 {
                                    file.flush().await?;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }

            let summary = DatasetSummaryRecord {
                record_type: DatasetRecordType::Summary,
                completed,
                duration_us: duration_to_u64(Instant::now().saturating_duration_since(started_at)),
                received_accepted_samples: counters_for_task
                    .received_accepted_samples
                    .load(Ordering::Relaxed),
                written_samples: counters_for_task.written_samples.load(Ordering::Relaxed),
                recorder_dropped_samples: counters_for_task
                    .recorder_dropped_samples
                    .load(Ordering::Relaxed),
            };

            write_record_line(&mut file, &summary).await?;
            file.flush().await?;
            Ok(summary)
        }
        .await;

        result
    });

    Ok(DatasetRecorderHandle {
        output_path: config.output_path,
        ingress,
        counters,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    })
}

pub fn default_dataset_output_path(
    scenario: RecordingScenario,
    now: SystemTime,
) -> io::Result<PathBuf> {
    let timestamp = format_dataset_timestamp(now)?;
    Ok(Path::new(DEFAULT_DATASET_DIRECTORY)
        .join(format!("{timestamp}-{}.ndjson", scenario.as_str())))
}

pub fn analyze_dataset_file(path: &Path) -> Result<DatasetAnalysis, DatasetAnalysisError> {
    let contents = fs::read_to_string(path)?;
    analyze_dataset_str(&contents)
}

pub fn analyze_dataset_str(contents: &str) -> Result<DatasetAnalysis, DatasetAnalysisError> {
    let mut metadata: Option<DatasetMetadataRecord> = None;
    let mut samples = Vec::new();
    let mut summary: Option<(usize, DatasetSummaryRecord)> = None;
    let mut previous_received_elapsed: Option<(usize, u64)> = None;
    let mut last_session_elapsed_by_session: HashMap<String, (usize, u64)> = HashMap::new();

    for (index, raw_line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let value: Value =
            serde_json::from_str(line).map_err(|err| DatasetAnalysisError::InvalidJsonLine {
                line: line_number,
                message: err.to_string(),
            })?;

        let record_type = value
            .get("recordType")
            .and_then(Value::as_str)
            .ok_or_else(|| DatasetAnalysisError::InvalidJsonLine {
                line: line_number,
                message: "recordType is required".to_owned(),
            })?;

        match record_type {
            "metadata" => {
                if summary.is_some() {
                    return Err(DatasetAnalysisError::InvalidDatasetStructure {
                        line: line_number,
                        message: "no records are allowed after summary".to_owned(),
                    });
                }
                if metadata.is_some() {
                    return Err(DatasetAnalysisError::InvalidDatasetStructure {
                        line: line_number,
                        message: "metadata record must appear exactly once".to_owned(),
                    });
                }
                if line_number != 1 {
                    return Err(DatasetAnalysisError::MissingMetadata);
                }

                let parsed: DatasetMetadataRecord =
                    serde_json::from_value(value.clone()).map_err(|err| {
                        DatasetAnalysisError::InvalidMetadata {
                            line: line_number,
                            message: err.to_string(),
                        }
                    })?;

                if parsed.dataset_format_version != DATASET_FORMAT_VERSION {
                    return Err(DatasetAnalysisError::UnsupportedDatasetVersion(
                        parsed.dataset_format_version,
                    ));
                }
                if parsed.protocol_version != PROTOCOL_VERSION {
                    return Err(DatasetAnalysisError::InvalidMetadata {
                        line: line_number,
                        message: format!(
                            "protocolVersion must be {PROTOCOL_VERSION}, got {}",
                            parsed.protocol_version
                        ),
                    });
                }
                OffsetDateTime::parse(&parsed.started_at_utc, &Rfc3339).map_err(|err| {
                    DatasetAnalysisError::InvalidMetadata {
                        line: line_number,
                        message: format!("startedAtUtc must be RFC 3339: {err}"),
                    }
                })?;

                metadata = Some(parsed);
            }
            "sample" => {
                if metadata.is_none() {
                    return Err(DatasetAnalysisError::MissingMetadata);
                }
                if summary.is_some() {
                    return Err(DatasetAnalysisError::InvalidDatasetStructure {
                        line: line_number,
                        message: "sample record is not allowed after summary".to_owned(),
                    });
                }
                let received_elapsed_us = value
                    .get("receivedElapsedUs")
                    .and_then(Value::as_u64)
                    .ok_or(DatasetAnalysisError::InvalidReceivedElapsedUs { line: line_number })?;
                if received_elapsed_us > MAX_ANALYZABLE_RECEIVED_ELAPSED_US {
                    return Err(DatasetAnalysisError::ReceivedElapsedUsOutOfRange {
                        line: line_number,
                        value: received_elapsed_us,
                        max_supported: MAX_ANALYZABLE_RECEIVED_ELAPSED_US,
                    });
                }
                if let Some((previous_line, previous_value)) = previous_received_elapsed {
                    if received_elapsed_us < previous_value {
                        return Err(DatasetAnalysisError::ReceivedElapsedRegression {
                            line: line_number,
                            previous_line,
                            previous_received_elapsed_us: previous_value,
                            current_received_elapsed_us: received_elapsed_us,
                        });
                    }
                }
                let sample_value = value.get("sample").cloned().ok_or_else(|| {
                    DatasetAnalysisError::InvalidMotionSample {
                        line: line_number,
                        message: "sample is required".to_owned(),
                    }
                })?;
                let sample: MotionSampleV1 =
                    serde_json::from_value(sample_value).map_err(|err| {
                        DatasetAnalysisError::InvalidMotionSample {
                            line: line_number,
                            message: err.to_string(),
                        }
                    })?;
                sample
                    .validate()
                    .map_err(|err| DatasetAnalysisError::InvalidMotionSample {
                        line: line_number,
                        message: err.to_string(),
                    })?;
                if let Some((previous_line, previous_value)) = last_session_elapsed_by_session
                    .get(&sample.session_id)
                    .copied()
                {
                    if sample.session_elapsed_us < previous_value {
                        return Err(DatasetAnalysisError::SessionElapsedRegression {
                            line: line_number,
                            previous_line,
                            session_id: sample.session_id.clone(),
                            previous_session_elapsed_us: previous_value,
                            current_session_elapsed_us: sample.session_elapsed_us,
                        });
                    }
                }
                previous_received_elapsed = Some((line_number, received_elapsed_us));
                last_session_elapsed_by_session.insert(
                    sample.session_id.clone(),
                    (line_number, sample.session_elapsed_us),
                );
                samples.push(DatasetSampleRecord {
                    record_type: DatasetRecordType::Sample,
                    received_elapsed_us,
                    sample,
                    line_number,
                });
            }
            "summary" => {
                if metadata.is_none() {
                    return Err(DatasetAnalysisError::MissingMetadata);
                }
                if summary.is_some() {
                    return Err(DatasetAnalysisError::InvalidDatasetStructure {
                        line: line_number,
                        message: "summary record must appear at most once".to_owned(),
                    });
                }
                if samples.is_empty() {
                    return Err(DatasetAnalysisError::InvalidDatasetStructure {
                        line: line_number,
                        message: "summary requires at least one preceding sample record".to_owned(),
                    });
                }
                let parsed: DatasetSummaryRecord =
                    serde_json::from_value(value).map_err(|err| {
                        DatasetAnalysisError::InvalidJsonLine {
                            line: line_number,
                            message: err.to_string(),
                        }
                    })?;
                summary = Some((line_number, parsed));
            }
            other => {
                return Err(DatasetAnalysisError::InvalidJsonLine {
                    line: line_number,
                    message: format!("unsupported recordType: {other}"),
                });
            }
        }
    }

    let metadata = metadata.ok_or(DatasetAnalysisError::MissingMetadata)?;
    if samples.is_empty() {
        return Err(DatasetAnalysisError::NoSamples);
    }

    validate_summary(summary.as_ref(), samples.len())?;

    build_analysis(metadata, samples, summary.map(|(_, record)| record))
}

pub fn format_human_analysis(analysis: &DatasetAnalysis, path: &Path) -> String {
    let mut out = String::new();
    let status = if analysis.is_complete {
        "complete"
    } else {
        "interrupted"
    };

    out.push_str(&format!(
        "Anchor Motion Dataset v{}\nfile: {}\nscenario: {}\nmountingConvention: {}\nstatus: {}\nsamples: {}\nsessions: {}\nobservedDurationUs: {}\nsourceAverageRateHz: {}\nreceiveAverageRateHz: {}\nrecorderDroppedSamples: {}\n",
        analysis.dataset_format_version,
        path.display(),
        analysis.scenario,
        analysis.mounting_convention,
        status,
        analysis.sample_count,
        analysis.session_count,
        analysis.observed_duration_us,
        format_f64(analysis.source_average_rate_hz),
        format_f64(analysis.receive_average_rate_hz),
        analysis.recorder_dropped_samples,
    ));

    out.push_str(&format!(
        "sourceIntervalUs: min={} mean={} p50={} p95={} p99={} max={}\n",
        option_u64(analysis.source_interval_us.min),
        option_f64(analysis.source_interval_us.mean),
        option_f64(analysis.source_interval_us.p50),
        option_f64(analysis.source_interval_us.p95),
        option_f64(analysis.source_interval_us.p99),
        option_u64(analysis.source_interval_us.max),
    ));
    out.push_str(&format!(
        "receiveIntervalUs: min={} mean={} p50={} p95={} p99={} max={}\n",
        option_u64(analysis.receive_interval_us.min),
        option_f64(analysis.receive_interval_us.mean),
        option_f64(analysis.receive_interval_us.p50),
        option_f64(analysis.receive_interval_us.p95),
        option_f64(analysis.receive_interval_us.p99),
        option_u64(analysis.receive_interval_us.max),
    ));
    out.push_str(&format!(
        "relativeDelayVariationUs: min={} mean={} p50={} p95={} p99={} max={}\n",
        option_i64(analysis.relative_delay_variation_us.min),
        option_f64(analysis.relative_delay_variation_us.mean),
        option_f64(analysis.relative_delay_variation_us.p50),
        option_f64(analysis.relative_delay_variation_us.p95),
        option_f64(analysis.relative_delay_variation_us.p99),
        option_i64(analysis.relative_delay_variation_us.max),
    ));
    append_axis_stats(
        &mut out,
        "linearAccelerationMps2.x (m/s^2)",
        &analysis.vectors.linear_acceleration_mps2.x,
    );
    append_axis_stats(
        &mut out,
        "linearAccelerationMps2.y (m/s^2)",
        &analysis.vectors.linear_acceleration_mps2.y,
    );
    append_axis_stats(
        &mut out,
        "linearAccelerationMps2.z (m/s^2)",
        &analysis.vectors.linear_acceleration_mps2.z,
    );
    append_axis_stats(
        &mut out,
        "gravityMps2.x (m/s^2)",
        &analysis.vectors.gravity_mps2.x,
    );
    append_axis_stats(
        &mut out,
        "gravityMps2.y (m/s^2)",
        &analysis.vectors.gravity_mps2.y,
    );
    append_axis_stats(
        &mut out,
        "gravityMps2.z (m/s^2)",
        &analysis.vectors.gravity_mps2.z,
    );
    append_axis_stats(
        &mut out,
        "angularVelocityRadS.x (rad/s)",
        &analysis.vectors.angular_velocity_rad_s.x,
    );
    append_axis_stats(
        &mut out,
        "angularVelocityRadS.y (rad/s)",
        &analysis.vectors.angular_velocity_rad_s.y,
    );
    append_axis_stats(
        &mut out,
        "angularVelocityRadS.z (rad/s)",
        &analysis.vectors.angular_velocity_rad_s.z,
    );
    append_axis_stats(
        &mut out,
        "gravityMagnitude (m/s^2)",
        &analysis.gravity_magnitude,
    );

    for session in &analysis.sessions {
        out.push_str(&format!(
            "session {}: samples={} firstSequence={} lastSequence={} gaps={} missingSequenceTotal={}\n",
            session.session_id,
            session.sample_count,
            session.first_sequence,
            session.last_sequence,
            session.gap_count,
            session.missing_sequence_total,
        ));
    }

    if !analysis.warnings.is_empty() {
        for warning in &analysis.warnings {
            out.push_str(&format!("warning: {warning}\n"));
        }
    }

    out.trim_end().to_owned()
}

fn build_analysis(
    metadata: DatasetMetadataRecord,
    samples: Vec<DatasetSampleRecord>,
    summary: Option<DatasetSummaryRecord>,
) -> Result<DatasetAnalysis, DatasetAnalysisError> {
    let mut session_summaries: Vec<SessionAccumulator> = Vec::new();
    let mut source_intervals = Vec::new();
    let mut receive_intervals = Vec::new();
    let mut relative_variations = Vec::new();

    let mut linear_x = Vec::new();
    let mut linear_y = Vec::new();
    let mut linear_z = Vec::new();
    let mut gravity_x = Vec::new();
    let mut gravity_y = Vec::new();
    let mut gravity_z = Vec::new();
    let mut angular_x = Vec::new();
    let mut angular_y = Vec::new();
    let mut angular_z = Vec::new();
    let mut gravity_magnitude = Vec::new();

    for sample in &samples {
        linear_x.push(sample.sample.linear_acceleration_mps2.x);
        linear_y.push(sample.sample.linear_acceleration_mps2.y);
        linear_z.push(sample.sample.linear_acceleration_mps2.z);
        gravity_x.push(sample.sample.gravity_mps2.x);
        gravity_y.push(sample.sample.gravity_mps2.y);
        gravity_z.push(sample.sample.gravity_mps2.z);
        angular_x.push(sample.sample.angular_velocity_rad_s.x);
        angular_y.push(sample.sample.angular_velocity_rad_s.y);
        angular_z.push(sample.sample.angular_velocity_rad_s.z);
        gravity_magnitude.push(vector_magnitude(
            sample.sample.gravity_mps2.x,
            sample.sample.gravity_mps2.y,
            sample.sample.gravity_mps2.z,
        ));

        if let Some(session) = session_summaries
            .iter_mut()
            .find(|candidate| candidate.session_id == sample.sample.session_id)
        {
            session.push(sample);
        } else {
            session_summaries.push(SessionAccumulator::new(sample));
        }
    }

    for session in &session_summaries {
        for pair in session.samples.windows(2) {
            let first = &pair[0];
            let second = &pair[1];
            let source_delta = second
                .sample
                .session_elapsed_us
                .checked_sub(first.sample.session_elapsed_us)
                .ok_or(DatasetAnalysisError::SessionElapsedRegression {
                    line: second.line_number,
                    previous_line: first.line_number,
                    session_id: second.sample.session_id.clone(),
                    previous_session_elapsed_us: first.sample.session_elapsed_us,
                    current_session_elapsed_us: second.sample.session_elapsed_us,
                })?;
            let receive_delta = second
                .received_elapsed_us
                .checked_sub(first.received_elapsed_us)
                .ok_or(DatasetAnalysisError::ReceivedElapsedRegression {
                    line: second.line_number,
                    previous_line: first.line_number,
                    previous_received_elapsed_us: first.received_elapsed_us,
                    current_received_elapsed_us: second.received_elapsed_us,
                })?;
            let receive_delta_i64 = i64::try_from(receive_delta).map_err(|_| {
                DatasetAnalysisError::NumericOverflow {
                    line: second.line_number,
                    message: format!("receive delta {receive_delta} cannot be represented as i64"),
                }
            })?;
            let source_delta_i64 =
                i64::try_from(source_delta).map_err(|_| DatasetAnalysisError::NumericOverflow {
                    line: second.line_number,
                    message: format!("source delta {source_delta} cannot be represented as i64"),
                })?;
            let relative_variation = receive_delta_i64.checked_sub(source_delta_i64).ok_or(
                DatasetAnalysisError::NumericOverflow {
                    line: second.line_number,
                    message: format!(
                        "relative delay variation overflowed for receive delta {receive_delta_i64} and source delta {source_delta_i64}"
                    ),
                },
            )?;
            source_intervals.push(source_delta);
            receive_intervals.push(receive_delta);
            relative_variations.push(relative_variation);
        }
    }

    let first_received = samples
        .first()
        .map(|sample| sample.received_elapsed_us)
        .unwrap_or(0);
    let last_received = samples
        .last()
        .map(|sample| sample.received_elapsed_us)
        .unwrap_or(0);
    let observed_duration_us = last_received.saturating_sub(first_received);

    let sessions: Vec<SessionAnalysis> = session_summaries
        .into_iter()
        .map(SessionAccumulator::finish)
        .collect::<Result<Vec<_>, _>>()?;
    let mut warnings = Vec::new();
    match summary.as_ref() {
        None => {
            warnings.push("dataset summary record is missing; file may be interrupted".to_owned())
        }
        Some(item) if !item.completed => warnings.push(
            "dataset summary indicates the recording ended early with completed=false".to_owned(),
        ),
        Some(_) => {}
    }

    Ok(DatasetAnalysis {
        dataset_format_version: metadata.dataset_format_version,
        protocol_version: metadata.protocol_version,
        scenario: metadata.scenario,
        mounting_convention: metadata.mounting_convention,
        is_complete: summary.as_ref().map(|item| item.completed).unwrap_or(false),
        sample_count: samples.len(),
        session_count: sessions.len(),
        observed_duration_us,
        source_average_rate_hz: average_rate_hz(&source_intervals),
        receive_average_rate_hz: average_rate_hz(&receive_intervals),
        source_interval_us: summarize_u64(&source_intervals),
        receive_interval_us: summarize_u64(&receive_intervals),
        relative_delay_variation_us: summarize_i64(&relative_variations),
        recorder_dropped_samples: summary
            .as_ref()
            .map(|item| item.recorder_dropped_samples)
            .unwrap_or(0),
        sessions,
        vectors: VectorFieldStats {
            linear_acceleration_mps2: AxisStats3 {
                x: summarize_scalar(&linear_x),
                y: summarize_scalar(&linear_y),
                z: summarize_scalar(&linear_z),
            },
            gravity_mps2: AxisStats3 {
                x: summarize_scalar(&gravity_x),
                y: summarize_scalar(&gravity_y),
                z: summarize_scalar(&gravity_z),
            },
            angular_velocity_rad_s: AxisStats3 {
                x: summarize_scalar(&angular_x),
                y: summarize_scalar(&angular_y),
                z: summarize_scalar(&angular_z),
            },
        },
        gravity_magnitude: summarize_scalar(&gravity_magnitude),
        warnings,
    })
}

fn summarize_u64(values: &[u64]) -> IntervalStats {
    if values.is_empty() {
        return IntervalStats {
            min: None,
            mean: None,
            p50: None,
            p95: None,
            p99: None,
            max: None,
        };
    }

    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    IntervalStats {
        min: sorted.first().copied(),
        mean: Some(mean_u64(values)),
        p50: Some(percentile_r7_u64(&sorted, 0.50)),
        p95: Some(percentile_r7_u64(&sorted, 0.95)),
        p99: Some(percentile_r7_u64(&sorted, 0.99)),
        max: sorted.last().copied(),
    }
}

fn summarize_i64(values: &[i64]) -> SignedIntervalStats {
    if values.is_empty() {
        return SignedIntervalStats {
            min: None,
            mean: None,
            p50: None,
            p95: None,
            p99: None,
            max: None,
        };
    }

    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    SignedIntervalStats {
        min: sorted.first().copied(),
        mean: Some(mean_i64(values)),
        p50: Some(percentile_r7_i64(&sorted, 0.50)),
        p95: Some(percentile_r7_i64(&sorted, 0.95)),
        p99: Some(percentile_r7_i64(&sorted, 0.99)),
        max: sorted.last().copied(),
    }
}

fn summarize_scalar(values: &[f64]) -> ScalarStats {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;

    for value in values {
        min = min.min(*value);
        max = max.max(*value);
        sum += *value;
    }

    let mean = sum / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / values.len() as f64;

    ScalarStats {
        min,
        max,
        mean,
        stddev: variance.sqrt(),
    }
}

fn average_rate_hz(intervals: &[u64]) -> f64 {
    if intervals.is_empty() {
        return 0.0;
    }

    let total_us: u128 = intervals.iter().map(|value| u128::from(*value)).sum();
    if total_us == 0 {
        return 0.0;
    }

    (intervals.len() as f64 * 1_000_000.0) / total_us as f64
}

fn mean_u64(values: &[u64]) -> f64 {
    values.iter().map(|value| *value as f64).sum::<f64>() / values.len() as f64
}

fn mean_i64(values: &[i64]) -> f64 {
    values.iter().map(|value| *value as f64).sum::<f64>() / values.len() as f64
}

fn percentile_r7_u64(sorted: &[u64], percentile: f64) -> f64 {
    percentile_r7(
        sorted.iter().map(|value| *value as f64).collect(),
        percentile,
    )
}

fn percentile_r7_i64(sorted: &[i64], percentile: f64) -> f64 {
    percentile_r7(
        sorted.iter().map(|value| *value as f64).collect(),
        percentile,
    )
}

fn percentile_r7(sorted: Vec<f64>, percentile: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }

    let rank = percentile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower_index = rank.floor() as usize;
    let upper_index = rank.ceil() as usize;
    if lower_index == upper_index {
        return sorted[lower_index];
    }

    let fraction = rank - lower_index as f64;
    sorted[lower_index] + (sorted[upper_index] - sorted[lower_index]) * fraction
}

fn vector_magnitude(x: f64, y: f64, z: f64) -> f64 {
    (x * x + y * y + z * z).sqrt()
}

fn format_started_at_utc(now: SystemTime) -> io::Result<String> {
    let utc = OffsetDateTime::from(now);
    utc.format(&Rfc3339)
        .map_err(|err| io::Error::other(format!("failed to format UTC time: {err}")))
}

fn format_dataset_timestamp(now: SystemTime) -> io::Result<String> {
    let utc = OffsetDateTime::from(now);
    Ok(format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        utc.year(),
        u8::from(utc.month()),
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second(),
    ))
}

fn duration_to_u64(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

async fn write_record_line<T: Serialize>(file: &mut tokio::fs::File, record: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(record)
        .map_err(|err| io::Error::other(format!("failed to serialize record: {err}")))?;
    bytes.push(b'\n');
    file.write_all(&bytes).await
}

async fn write_sample_event(
    file: &mut tokio::fs::File,
    counters: &RecorderCounters,
    started_at: Instant,
    event: AcceptedSampleEvent,
    flow_control: Option<&DatasetRecorderFlowControl>,
) -> io::Result<()> {
    if let Some(flow_control) = flow_control {
        flow_control.wait_until_released_for_sample_write().await;
    }
    let record = DatasetSampleRecord {
        record_type: DatasetRecordType::Sample,
        received_elapsed_us: duration_to_u64(
            event.received_at.saturating_duration_since(started_at),
        ),
        sample: event.sample,
        line_number: 0,
    };
    write_record_line(file, &record).await?;
    counters.written_samples.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

fn option_f64(value: Option<f64>) -> String {
    value.map(format_f64).unwrap_or_else(|| "n/a".to_owned())
}

fn option_u64(value: Option<u64>) -> String {
    value
        .map(|inner| inner.to_string())
        .unwrap_or_else(|| "n/a".to_owned())
}

fn option_i64(value: Option<i64>) -> String {
    value
        .map(|inner| inner.to_string())
        .unwrap_or_else(|| "n/a".to_owned())
}

fn format_f64(value: f64) -> String {
    format!("{value:.3}")
}

fn append_axis_stats(out: &mut String, label: &str, stats: &ScalarStats) {
    out.push_str(&format!(
        "{}: min={} max={} mean={} stddev={}\n",
        label,
        format_f64(stats.min),
        format_f64(stats.max),
        format_f64(stats.mean),
        format_f64(stats.stddev),
    ));
}

fn validate_summary(
    summary: Option<&(usize, DatasetSummaryRecord)>,
    actual_sample_count: usize,
) -> Result<(), DatasetAnalysisError> {
    let Some((line_number, summary)) = summary else {
        return Ok(());
    };

    let actual_sample_count_u64 =
        u64::try_from(actual_sample_count).map_err(|_| DatasetAnalysisError::InvalidSummary {
            line: *line_number,
            message: "sample count does not fit in u64 while validating summary".to_owned(),
        })?;
    if summary.written_samples != actual_sample_count_u64 {
        return Err(DatasetAnalysisError::InvalidSummary {
            line: *line_number,
            message: format!(
                "writtenSamples={} but dataset contains {} sample records",
                summary.written_samples, actual_sample_count_u64
            ),
        });
    }

    let expected_received = summary
        .written_samples
        .checked_add(summary.recorder_dropped_samples)
        .ok_or_else(|| DatasetAnalysisError::InvalidSummary {
            line: *line_number,
            message: "receivedAcceptedSamples overflowed while validating summary".to_owned(),
        })?;
    if summary.received_accepted_samples != expected_received {
        return Err(DatasetAnalysisError::InvalidSummary {
            line: *line_number,
            message: format!(
                "receivedAcceptedSamples={} but writtenSamples + recorderDroppedSamples = {}",
                summary.received_accepted_samples, expected_received
            ),
        });
    }

    Ok(())
}

#[derive(Debug)]
struct SessionAccumulator<'a> {
    session_id: String,
    samples: Vec<&'a DatasetSampleRecord>,
}

impl<'a> SessionAccumulator<'a> {
    fn new(sample: &'a DatasetSampleRecord) -> Self {
        Self {
            session_id: sample.sample.session_id.clone(),
            samples: vec![sample],
        }
    }

    fn push(&mut self, sample: &'a DatasetSampleRecord) {
        self.samples.push(sample);
    }

    fn finish(self) -> Result<SessionAnalysis, DatasetAnalysisError> {
        let first_sequence = self
            .samples
            .first()
            .map(|sample| sample.sample.sequence)
            .unwrap_or(0);
        let last_sequence = self
            .samples
            .last()
            .map(|sample| sample.sample.sequence)
            .unwrap_or(0);
        let mut gaps = Vec::new();
        let mut missing_sequence_total = 0_u64;

        for pair in self.samples.windows(2) {
            let current = pair[0].sample.sequence;
            let next = pair[1].sample.sequence;
            if let Some(sequence_gap) = next.checked_sub(current) {
                if sequence_gap > 1 {
                    let missing_count = u64::from(sequence_gap - 1);
                    missing_sequence_total = missing_sequence_total.checked_add(missing_count).ok_or(
                        DatasetAnalysisError::NumericOverflow {
                            line: pair[1].line_number,
                            message: format!(
                                "missing sequence total overflowed while accumulating gap after {} before {}",
                                current, next
                            ),
                        },
                    )?;
                    gaps.push(SequenceGap {
                        after_sequence: current,
                        before_sequence: next,
                        missing_count,
                    });
                }
            }
        }

        Ok(SessionAnalysis {
            session_id: self.session_id,
            sample_count: self.samples.len(),
            first_sequence,
            last_sequence,
            gap_count: gaps.len(),
            missing_sequence_total,
            gaps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPLETE_FIXTURE: &str =
        include_str!("../../test-fixtures/motion-datasets/complete-basic.ndjson");
    const INTERRUPTED_FIXTURE: &str =
        include_str!("../../test-fixtures/motion-datasets/interrupted-basic.ndjson");

    #[test]
    fn metadata_serialization_is_deterministic() {
        let record = DatasetMetadataRecord {
            record_type: DatasetRecordType::Metadata,
            dataset_format_version: 1,
            protocol_version: 1,
            scenario: RecordingScenario::Stationary,
            started_at_utc: "2026-09-02T12:00:00Z".to_owned(),
            expected_sample_rate_hz: 60,
            mounting_convention: DEFAULT_MOUNTING_CONVENTION.to_owned(),
            notes: None,
        };

        let serialized = serde_json::to_string(&record).expect("serialize metadata");
        assert_eq!(
            serialized,
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}"
        );
    }

    #[test]
    fn summary_serialization_is_deterministic() {
        let record = DatasetSummaryRecord {
            record_type: DatasetRecordType::Summary,
            completed: true,
            duration_us: 330_000,
            received_accepted_samples: 3,
            written_samples: 3,
            recorder_dropped_samples: 0,
        };

        let serialized = serde_json::to_string(&record).expect("serialize summary");
        assert_eq!(
            serialized,
            "{\"recordType\":\"summary\",\"completed\":true,\"durationUs\":330000,\"receivedAcceptedSamples\":3,\"writtenSamples\":3,\"recorderDroppedSamples\":0}"
        );
    }

    #[test]
    fn analyzer_detects_interrupted_dataset() {
        let analysis = analyze_dataset_str(INTERRUPTED_FIXTURE).expect("analyze interrupted");
        assert!(!analysis.is_complete);
        assert_eq!(analysis.warnings.len(), 1);
    }

    #[test]
    fn analyzer_rejects_missing_metadata() {
        let err = analyze_dataset_str("{\"recordType\":\"sample\"}\n")
            .expect_err("metadata must be required");
        assert!(matches!(err, DatasetAnalysisError::MissingMetadata));
    }

    #[test]
    fn analyzer_rejects_invalid_received_elapsed_us() {
        let err = analyze_dataset_str(concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":-1,\"sample\":{}}\n"
        ))
        .expect_err("receivedElapsedUs must be validated");

        assert!(matches!(
            err,
            DatasetAnalysisError::InvalidReceivedElapsedUs { line: 2 }
        ));
    }

    #[test]
    fn analyzer_accepts_received_elapsed_us_at_i64_max() {
        let analysis = analyze_dataset_str(concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":9223372036854775807,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
        ))
        .expect("i64::MAX receivedElapsedUs should analyze");

        assert_eq!(analysis.sample_count, 1);
    }

    #[test]
    fn analyzer_rejects_received_elapsed_us_above_i64_max_without_panicking() {
        let panic_result = std::panic::catch_unwind(|| {
            analyze_dataset_str(concat!(
                "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
                "{\"recordType\":\"sample\",\"receivedElapsedUs\":9223372036854775808,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
            ))
        });

        let result = panic_result.expect("analysis must not panic");
        let err = result.expect_err("value above i64::MAX must fail");
        assert!(matches!(
            err,
            DatasetAnalysisError::ReceivedElapsedUsOutOfRange {
                line: 2,
                value: 9_223_372_036_854_775_808,
                max_supported: 9_223_372_036_854_775_807,
            }
        ));
    }

    #[test]
    fn analyzer_rejects_invalid_motion_sample() {
        let err = analyze_dataset_str(concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":0,\"sample\":{\"protocolVersion\":1}}\n"
        ))
        .expect_err("sample must be validated");

        assert!(matches!(
            err,
            DatasetAnalysisError::InvalidMotionSample { line: 2, .. }
        ));
    }

    #[test]
    fn analyzer_rejects_empty_dataset() {
        let err = analyze_dataset_str(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
        )
        .expect_err("dataset without samples must fail");

        assert!(matches!(err, DatasetAnalysisError::NoSamples));
    }

    #[test]
    fn analyzer_calculates_known_statistics() {
        let analysis = analyze_dataset_str(COMPLETE_FIXTURE).expect("complete fixture");

        assert_eq!(analysis.sample_count, 3);
        assert_eq!(analysis.session_count, 1);
        assert_eq!(analysis.observed_duration_us, 330_000);
        assert_eq!(analysis.source_interval_us.min, Some(100_000));
        assert_eq!(analysis.source_interval_us.max, Some(200_000));
        assert_eq!(analysis.receive_interval_us.min, Some(120_000));
        assert_eq!(analysis.receive_interval_us.max, Some(210_000));
        assert_eq!(analysis.relative_delay_variation_us.min, Some(10_000));
        assert_eq!(analysis.relative_delay_variation_us.max, Some(20_000));
        assert!((analysis.source_interval_us.mean.expect("mean") - 150_000.0).abs() < 1e-9);
        assert!((analysis.source_interval_us.p50.expect("p50") - 150_000.0).abs() < 1e-9);
        assert!((analysis.source_interval_us.p95.expect("p95") - 195_000.0).abs() < 1e-9);
        assert!((analysis.source_interval_us.p99.expect("p99") - 199_000.0).abs() < 1e-9);
        assert!((analysis.source_average_rate_hz - 6.666666666666667).abs() < 1e-9);
        assert!((analysis.receive_average_rate_hz - 6.0606060606060606).abs() < 1e-9);
        assert_eq!(analysis.sessions[0].gap_count, 1);
        assert_eq!(analysis.sessions[0].missing_sequence_total, 1);
        assert!((analysis.vectors.linear_acceleration_mps2.x.mean - 0.0).abs() < 1e-9);
        assert!((analysis.vectors.linear_acceleration_mps2.y.mean - 2.0).abs() < 1e-9);
        assert!((analysis.vectors.linear_acceleration_mps2.z.mean - 3.0).abs() < 1e-9);
        assert!((analysis.gravity_magnitude.mean - 9.8).abs() < 1e-12);
    }

    #[test]
    fn analyzer_handles_relative_delay_variation_near_numeric_extremes() {
        let analysis = analyze_dataset_str(concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":0,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":9223372036854775807,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":1,\"sessionElapsedUs\":9007199254740991,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
        ))
        .expect("extreme relative variation should analyze");

        assert_eq!(
            analysis.relative_delay_variation_us.max,
            Some(9_214_364_837_600_034_816)
        );
    }

    #[test]
    fn average_rate_hz_does_not_overflow_when_interval_sum_exceeds_u64() {
        let panic_result = std::panic::catch_unwind(|| average_rate_hz(&[u64::MAX, u64::MAX]));
        let rate = panic_result.expect("average_rate_hz must not panic");

        assert!(rate.is_finite());
        assert!(rate > 0.0);
    }

    #[test]
    fn human_output_includes_all_vector_axis_and_gravity_magnitude_stats() {
        let analysis = analyze_dataset_str(COMPLETE_FIXTURE).expect("complete fixture");
        let rendered = format_human_analysis(&analysis, Path::new("fixture.ndjson"));

        assert!(rendered.contains("linearAccelerationMps2.x (m/s^2): min="));
        assert!(rendered.contains("linearAccelerationMps2.y (m/s^2): min="));
        assert!(rendered.contains("linearAccelerationMps2.z (m/s^2): min="));
        assert!(rendered.contains("gravityMps2.x (m/s^2): min="));
        assert!(rendered.contains("gravityMps2.y (m/s^2): min="));
        assert!(rendered.contains("gravityMps2.z (m/s^2): min="));
        assert!(rendered.contains("angularVelocityRadS.x (rad/s): min="));
        assert!(rendered.contains("angularVelocityRadS.y (rad/s): min="));
        assert!(rendered.contains("angularVelocityRadS.z (rad/s): min="));
        assert!(rendered.contains("gravityMagnitude (m/s^2): min="));
        assert!(rendered.contains("stddev="));
    }

    #[test]
    fn analyzer_rejects_metadata_after_summary() {
        let err = analyze_dataset_str(concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":0,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n",
            "{\"recordType\":\"summary\",\"completed\":true,\"durationUs\":0,\"receivedAcceptedSamples\":1,\"writtenSamples\":1,\"recorderDroppedSamples\":0}\n",
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n"
        ))
        .expect_err("metadata after summary must fail");

        assert!(matches!(
            err,
            DatasetAnalysisError::InvalidDatasetStructure { line: 4, .. }
        ));
    }

    #[test]
    fn analyzer_handles_single_sample_dataset() {
        let analysis = analyze_dataset_str(concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":0,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":5,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":1.0,\"y\":2.0,\"z\":3.0},\"gravityMps2\":{\"x\":0.0,\"y\":0.0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0.0,\"y\":0.0,\"z\":0.0}}}\n"
        ))
        .expect("single-sample dataset");

        assert_eq!(analysis.sample_count, 1);
        assert_eq!(analysis.source_average_rate_hz, 0.0);
        assert_eq!(analysis.receive_average_rate_hz, 0.0);
        assert_eq!(analysis.source_interval_us.min, None);
        assert_eq!(analysis.receive_interval_us.min, None);
        assert_eq!(analysis.relative_delay_variation_us.min, None);
        assert_eq!(analysis.gravity_magnitude.stddev, 0.0);
    }

    #[test]
    fn analyzer_tracks_multiple_sessions() {
        let analysis = analyze_dataset_str(INTERRUPTED_FIXTURE).expect("interrupted fixture");

        assert_eq!(analysis.session_count, 2);
        assert_eq!(analysis.sessions[0].session_id, "session-a");
        assert_eq!(analysis.sessions[1].session_id, "session-b");
    }

    #[test]
    fn default_output_path_uses_relative_artifact_directory() {
        let path = default_dataset_output_path(
            RecordingScenario::Stationary,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_788_350_400),
        )
        .expect("default path");

        assert_eq!(
            path,
            Path::new("artifacts/motion-datasets/20260902T120000Z-stationary.ndjson")
        );
    }
}
