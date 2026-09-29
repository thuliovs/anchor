use crate::{
    calibration::load_calibration_profile_file,
    live_motion::{LiveMotionPipeline, LiveTiltSnapshotV1, NeutralReason},
    motion_filtering::selection::TiltEstimatorPolicyV2,
    receiver::{AcceptedSampleEvent, AcceptedSampleSink},
};
use serde::Serialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const CALIBRATION_PROFILE_PATH_ENV: &str = "ANCHOR_CALIBRATION_PROFILE_PATH";
pub const TILT_POLICY_PATH_ENV: &str = "ANCHOR_TILT_POLICY_PATH";
/// Bounded ingress capacity for accepted UDP samples. At 60 Hz this buffers about two seconds.
pub const LIVE_MOTION_INGRESS_CAPACITY: usize = 120;

pub type SharedLiveTiltRuntime = Arc<LiveTiltRuntime>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTiltIntegrationMetrics {
    pub ingress_dropped_events: u64,
    pub ingress_closed_events: u64,
    pub processed_events: u64,
}

#[derive(Debug, Default)]
pub struct LiveTiltIntegrationCounters {
    ingress_dropped_events: AtomicU64,
    ingress_closed_events: AtomicU64,
    processed_events: AtomicU64,
}

impl LiveTiltIntegrationCounters {
    fn snapshot(&self) -> LiveTiltIntegrationMetrics {
        LiveTiltIntegrationMetrics {
            ingress_dropped_events: self.ingress_dropped_events.load(Ordering::Relaxed),
            ingress_closed_events: self.ingress_closed_events.load(Ordering::Relaxed),
            processed_events: self.processed_events.load(Ordering::Relaxed),
        }
    }

    fn increment_dropped(&self) {
        saturating_fetch_add(&self.ingress_dropped_events);
    }

    fn increment_closed(&self) {
        saturating_fetch_add(&self.ingress_closed_events);
    }

    fn increment_processed(&self) {
        saturating_fetch_add(&self.processed_events);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTiltRuntimeSnapshotV1 {
    #[serde(flatten)]
    pub pipeline: LiveTiltSnapshotV1,
    pub integration_metrics: LiveTiltIntegrationMetrics,
}

#[derive(Debug)]
pub struct LiveTiltRuntimeState {
    pipeline: LiveMotionPipeline,
}

impl LiveTiltRuntimeState {
    pub fn new(pipeline: LiveMotionPipeline) -> Self {
        Self { pipeline }
    }

    pub fn unavailable(reason: NeutralReason, error: Option<String>) -> Self {
        let pipeline = error.map_or_else(
            || LiveMotionPipeline::unavailable(reason),
            |error| LiveMotionPipeline::unavailable_with_runtime_error(reason, error),
        );
        Self::new(pipeline)
    }

    pub fn process(&mut self, event: AcceptedSampleEvent) {
        self.pipeline.process(event);
    }

    pub fn snapshot(&self, now: Instant) -> LiveTiltSnapshotV1 {
        self.pipeline.snapshot(now)
    }
}

#[derive(Debug)]
pub struct LiveTiltRuntime {
    state: Mutex<LiveTiltRuntimeState>,
    integration_counters: Arc<LiveTiltIntegrationCounters>,
}

impl LiveTiltRuntime {
    fn new(pipeline: LiveMotionPipeline) -> Self {
        Self {
            state: Mutex::new(LiveTiltRuntimeState::new(pipeline)),
            integration_counters: Arc::new(LiveTiltIntegrationCounters::default()),
        }
    }

    fn snapshot(&self, now: Instant) -> Result<LiveTiltRuntimeSnapshotV1, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "live tilt runtime state is unavailable".to_owned())?;
        Ok(LiveTiltRuntimeSnapshotV1 {
            pipeline: state.snapshot(now),
            integration_metrics: self.integration_counters.snapshot(),
        })
    }
}

#[derive(Clone)]
pub struct LiveTiltIngress {
    tx: mpsc::SyncSender<AcceptedSampleEvent>,
    counters: Arc<LiveTiltIntegrationCounters>,
}

impl AcceptedSampleSink for LiveTiltIngress {
    fn try_publish(&self, event: AcceptedSampleEvent) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => self.counters.increment_dropped(),
            Err(mpsc::TrySendError::Disconnected(_)) => self.counters.increment_closed(),
        }
    }
}

pub struct LiveTiltRuntimeHandle {
    pub state: SharedLiveTiltRuntime,
    pub sink: Arc<dyn AcceptedSampleSink>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    join_handle: thread::JoinHandle<()>,
}

impl LiveTiltRuntimeHandle {
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Err(err) = self.join_handle.join() {
            eprintln!("live tilt runtime shutdown join error: {err:?}");
        }
    }
}

pub fn read_live_tilt_snapshot(
    state: &SharedLiveTiltRuntime,
    now: Instant,
) -> Result<LiveTiltRuntimeSnapshotV1, String> {
    state.snapshot(now)
}

pub fn start_live_tilt_runtime_from_env() -> LiveTiltRuntimeHandle {
    start_live_tilt_runtime(load_pipeline_from_env())
}

pub fn start_live_tilt_runtime(pipeline: LiveMotionPipeline) -> LiveTiltRuntimeHandle {
    let runtime = Arc::new(LiveTiltRuntime::new(pipeline));
    let (tx, rx) = mpsc::sync_channel(LIVE_MOTION_INGRESS_CAPACITY);
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let task_runtime = runtime.clone();
    let join_handle = thread::spawn(move || loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }

        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(event) => match task_runtime.state.lock() {
                Ok(mut state) => {
                    state.process(event);
                    task_runtime.integration_counters.increment_processed();
                }
                Err(_) => {
                    eprintln!("live tilt runtime lock poisoned while processing event");
                    break;
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    });
    let sink = Arc::new(LiveTiltIngress {
        tx,
        counters: runtime.integration_counters.clone(),
    });
    LiveTiltRuntimeHandle {
        state: runtime,
        sink,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    }
}

pub fn load_pipeline_from_env() -> LiveMotionPipeline {
    let config = LiveTiltRuntimeConfig::from_env();
    match load_pipeline_from_config(config) {
        Ok(pipeline) => pipeline,
        Err(err) => {
            eprintln!("live tilt pipeline unavailable at startup: {err}");
            LiveMotionPipeline::unavailable_with_runtime_error(err.reason, err.to_string())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTiltRuntimeConfig {
    pub calibration_profile_path: Option<PathBuf>,
    pub tilt_policy_path: Option<PathBuf>,
}

impl LiveTiltRuntimeConfig {
    pub fn new(
        calibration_profile_path: Option<PathBuf>,
        tilt_policy_path: Option<PathBuf>,
    ) -> Self {
        Self {
            calibration_profile_path,
            tilt_policy_path,
        }
    }

    pub fn from_env() -> Self {
        Self::new(
            env::var_os(CALIBRATION_PROFILE_PATH_ENV).map(PathBuf::from),
            env::var_os(TILT_POLICY_PATH_ENV).map(PathBuf::from),
        )
    }
}

#[derive(Debug)]
pub struct RuntimeConfigError {
    pub reason: NeutralReason,
    message: String,
}

impl std::fmt::Display for RuntimeConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub fn load_pipeline_from_config(
    config: LiveTiltRuntimeConfig,
) -> Result<LiveMotionPipeline, RuntimeConfigError> {
    let profile_path = config
        .calibration_profile_path
        .ok_or_else(|| RuntimeConfigError {
            reason: NeutralReason::MissingConfiguration,
            message: format!(
                "required environment variable {CALIBRATION_PROFILE_PATH_ENV} is not set"
            ),
        })?;
    let policy_path = config.tilt_policy_path.ok_or_else(|| RuntimeConfigError {
        reason: NeutralReason::MissingConfiguration,
        message: format!("required environment variable {TILT_POLICY_PATH_ENV} is not set"),
    })?;

    load_pipeline_from_paths(&profile_path, &policy_path)
}

pub fn load_pipeline_from_paths(
    profile_path: &Path,
    policy_path: &Path,
) -> Result<LiveMotionPipeline, RuntimeConfigError> {
    let profile =
        load_calibration_profile_file(profile_path).map_err(|err| RuntimeConfigError {
            reason: NeutralReason::InvalidProfile,
            message: format!(
                "invalid calibration profile at {}: {err}",
                profile_path.display()
            ),
        })?;
    let policy_contents = fs::read_to_string(policy_path).map_err(|err| RuntimeConfigError {
        reason: NeutralReason::InvalidPolicy,
        message: format!(
            "failed to read tilt policy at {}: {err}",
            policy_path.display()
        ),
    })?;
    let policy: TiltEstimatorPolicyV2 =
        serde_json::from_str(&policy_contents).map_err(|err| RuntimeConfigError {
            reason: NeutralReason::InvalidPolicy,
            message: format!(
                "invalid TiltEstimatorPolicyV2 at {}: {err}",
                policy_path.display()
            ),
        })?;
    policy.validate().map_err(|err| RuntimeConfigError {
        reason: NeutralReason::InvalidPolicy,
        message: format!(
            "invalid TiltEstimatorPolicyV2 at {}: {err}",
            policy_path.display()
        ),
    })?;
    Ok(LiveMotionPipeline::new(&profile, policy))
}

fn saturating_fetch_add(value: &AtomicU64) {
    let mut current = value.load(Ordering::Relaxed);
    loop {
        if current == u64::MAX {
            return;
        }
        match value.compare_exchange_weak(
            current,
            current.saturating_add(1),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(next) => current = next,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        live_motion::{CalibrationProfileFingerprintV1, LiveMotionState},
        motion_filtering::{
            selection::{PolicyCandidate, TiltEstimatorPolicyV1, POLICY_SOURCE},
            synthetic::synthetic_suite,
        },
        receiver::{udp::start_udp_receiver, ReceiverState, SharedReceiverState},
    };
    use std::{
        collections::BTreeMap,
        net::SocketAddr,
        sync::{mpsc as std_mpsc, Arc as StdArc, Barrier},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use tokio::sync::watch;

    fn fixture_profile(index: usize) -> crate::calibration::CalibrationProfileV1 {
        synthetic_suite().unwrap().remove(index).calibration_profile
    }

    fn policy(profile: &crate::calibration::CalibrationProfileV1) -> TiltEstimatorPolicyV2 {
        TiltEstimatorPolicyV2 {
            version: 2,
            candidate: PolicyCandidate::GravityNoAdditionalAnchorFilter,
            parameters: BTreeMap::new(),
            yaw_available: false,
            source: POLICY_SOURCE.to_owned(),
            calibration_profile_fingerprint: CalibrationProfileFingerprintV1::for_profile(profile)
                .unwrap(),
        }
    }

    fn event(seq: u32, elapsed: u64, at: Instant) -> AcceptedSampleEvent {
        let mut sample = synthetic_suite().unwrap()[0].raw_samples[0].clone();
        sample.sequence = seq;
        sample.session_elapsed_us = elapsed;
        AcceptedSampleEvent {
            sample,
            sender: "127.0.0.1:1".parse::<SocketAddr>().unwrap(),
            received_at: at,
        }
    }

    fn assert_send<T: Send>() {}

    fn temp_dir() -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("anchor-live-tilt-runtime-{id}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_json<T: Serialize>(dir: &Path, name: &str, value: &T) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
        path
    }

    fn snapshot_for_pipeline(pipeline: LiveMotionPipeline) -> LiveTiltSnapshotV1 {
        pipeline.snapshot(Instant::now())
    }

    fn unavailable_from_config(config: LiveTiltRuntimeConfig) -> LiveTiltSnapshotV1 {
        match load_pipeline_from_config(config) {
            Ok(pipeline) => snapshot_for_pipeline(pipeline),
            Err(err) => {
                LiveMotionPipeline::unavailable_with_runtime_error(err.reason, err.to_string())
                    .snapshot(Instant::now())
            }
        }
    }

    #[test]
    fn runtime_state_is_send() {
        assert_send::<LiveTiltRuntimeState>();
        assert_send::<LiveTiltRuntime>();
    }

    #[test]
    fn unavailable_snapshot_is_neutral() {
        let state = LiveTiltRuntimeState::unavailable(
            NeutralReason::MissingConfiguration,
            Some("missing".into()),
        );
        let snap = state.snapshot(Instant::now());
        assert_eq!(snap.validity, LiveMotionState::Unavailable);
        assert_eq!(snap.target_tilt.roll_rad, 0.0);
    }

    #[test]
    fn snapshot_read_is_non_mutating() {
        let state = Arc::new(LiveTiltRuntime::new(LiveMotionPipeline::unavailable(
            NeutralReason::MissingConfiguration,
        )));
        let a = read_live_tilt_snapshot(&state, Instant::now()).unwrap();
        let b = read_live_tilt_snapshot(&state, Instant::now()).unwrap();
        assert_eq!(a.integration_metrics, b.integration_metrics);
    }

    #[test]
    fn poisoned_lock_returns_controlled_error() {
        let state = Arc::new(LiveTiltRuntime::new(LiveMotionPipeline::unavailable(
            NeutralReason::MissingConfiguration,
        )));
        let cloned = state.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = cloned.state.lock().unwrap();
            panic!("poison");
        });
        assert_eq!(
            read_live_tilt_snapshot(&state, Instant::now()).unwrap_err(),
            "live tilt runtime state is unavailable"
        );
    }

    #[tokio::test]
    async fn accepted_event_updates_pipeline() {
        let profile = fixture_profile(0);
        let handle = start_live_tilt_runtime(LiveMotionPipeline::new(&profile, policy(&profile)));
        let now = Instant::now();
        handle.sink.try_publish(event(1, 10_000, now));
        handle
            .sink
            .try_publish(event(2, 20_000, now + Duration::from_millis(10)));
        handle
            .sink
            .try_publish(event(3, 30_000, now + Duration::from_millis(20)));
        tokio::time::sleep(Duration::from_millis(20)).await;
        let snap = read_live_tilt_snapshot(&handle.state, now + Duration::from_millis(21)).unwrap();
        assert!(snap.integration_metrics.processed_events >= 3);
        assert_eq!(snap.pipeline.last_sequence, Some(3));
        handle.shutdown().await;
    }

    #[test]
    fn full_queue_records_drop_without_waiting_for_pipeline_mutex() {
        let runtime = Arc::new(LiveTiltRuntime::new(LiveMotionPipeline::unavailable(
            NeutralReason::MissingConfiguration,
        )));
        let guard = runtime.state.lock().unwrap();
        let (tx, _rx) = mpsc::sync_channel(1);
        let sink = LiveTiltIngress {
            tx,
            counters: runtime.integration_counters.clone(),
        };
        let now = Instant::now();
        sink.try_publish(event(1, 1, now));

        let barrier = StdArc::new(Barrier::new(2));
        let thread_barrier = barrier.clone();
        let (done_tx, done_rx) = std_mpsc::channel();
        let worker = std::thread::spawn(move || {
            thread_barrier.wait();
            sink.try_publish(event(2, 2, now));
            done_tx.send(()).unwrap();
        });

        barrier.wait();
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert_eq!(
            runtime
                .integration_counters
                .snapshot()
                .ingress_dropped_events,
            1
        );
        worker.join().unwrap();
        drop(guard);
    }

    #[test]
    fn closed_queue_records_without_waiting_for_pipeline_mutex() {
        let runtime = Arc::new(LiveTiltRuntime::new(LiveMotionPipeline::unavailable(
            NeutralReason::MissingConfiguration,
        )));
        let guard = runtime.state.lock().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        drop(rx);
        let sink = LiveTiltIngress {
            tx,
            counters: runtime.integration_counters.clone(),
        };

        let barrier = StdArc::new(Barrier::new(2));
        let thread_barrier = barrier.clone();
        let (done_tx, done_rx) = std_mpsc::channel();
        let worker = std::thread::spawn(move || {
            thread_barrier.wait();
            sink.try_publish(event(1, 1, Instant::now()));
            done_tx.send(()).unwrap();
        });

        barrier.wait();
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert_eq!(
            runtime
                .integration_counters
                .snapshot()
                .ingress_closed_events,
            1
        );
        worker.join().unwrap();
        drop(guard);
    }

    #[tokio::test]
    async fn shutdown_finishes() {
        let handle = start_live_tilt_runtime(LiveMotionPipeline::unavailable(
            NeutralReason::MissingConfiguration,
        ));
        handle.shutdown().await;
    }

    #[test]
    fn missing_both_paths_is_missing_configuration() {
        let snap = unavailable_from_config(LiveTiltRuntimeConfig::new(None, None));
        assert_eq!(snap.neutral_reason, NeutralReason::MissingConfiguration);
    }

    #[test]
    fn only_profile_path_is_missing_configuration() {
        let snap = unavailable_from_config(LiveTiltRuntimeConfig::new(
            Some(PathBuf::from("profile.json")),
            None,
        ));
        assert_eq!(snap.neutral_reason, NeutralReason::MissingConfiguration);
    }

    #[test]
    fn only_policy_path_is_missing_configuration() {
        let snap = unavailable_from_config(LiveTiltRuntimeConfig::new(
            None,
            Some(PathBuf::from("policy.json")),
        ));
        assert_eq!(snap.neutral_reason, NeutralReason::MissingConfiguration);
    }

    #[test]
    fn missing_profile_path_is_invalid_profile() {
        let dir = temp_dir();
        let policy_path = write_json(&dir, "policy.json", &policy(&fixture_profile(0)));
        let err = load_pipeline_from_paths(&dir.join("missing-profile.json"), &policy_path)
            .expect_err("missing profile should fail");
        assert_eq!(err.reason, NeutralReason::InvalidProfile);
    }

    #[test]
    fn invalid_profile_file_is_invalid_profile() {
        let dir = temp_dir();
        let profile_path = dir.join("profile.json");
        fs::write(&profile_path, "not-json").unwrap();
        let policy_path = write_json(&dir, "policy.json", &policy(&fixture_profile(0)));
        let err = load_pipeline_from_paths(&profile_path, &policy_path)
            .expect_err("invalid profile should fail");
        assert_eq!(err.reason, NeutralReason::InvalidProfile);
    }

    #[test]
    fn missing_policy_path_is_invalid_policy() {
        let dir = temp_dir();
        let profile = fixture_profile(0);
        let profile_path = write_json(&dir, "profile.json", &profile);
        let err = load_pipeline_from_paths(&profile_path, &dir.join("missing-policy.json"))
            .expect_err("missing policy should fail");
        assert_eq!(err.reason, NeutralReason::InvalidPolicy);
    }

    #[test]
    fn invalid_policy_json_is_invalid_policy() {
        let dir = temp_dir();
        let profile = fixture_profile(0);
        let profile_path = write_json(&dir, "profile.json", &profile);
        let policy_path = dir.join("policy.json");
        fs::write(&policy_path, "not-json").unwrap();
        let err = load_pipeline_from_paths(&profile_path, &policy_path)
            .expect_err("invalid policy should fail");
        assert_eq!(err.reason, NeutralReason::InvalidPolicy);
    }

    #[test]
    fn policy_v1_is_rejected() {
        let dir = temp_dir();
        let profile = fixture_profile(0);
        let profile_path = write_json(&dir, "profile.json", &profile);
        let policy_v1 = TiltEstimatorPolicyV1 {
            version: 1,
            candidate: PolicyCandidate::GravityNoAdditionalAnchorFilter,
            parameters: BTreeMap::new(),
            yaw_available: false,
            source: POLICY_SOURCE.to_owned(),
        };
        let policy_path = write_json(&dir, "policy-v1.json", &policy_v1);
        let err = load_pipeline_from_paths(&profile_path, &policy_path)
            .expect_err("policy v1 should fail");
        assert_eq!(err.reason, NeutralReason::InvalidPolicy);
    }

    #[test]
    fn policy_v2_with_unknown_fields_is_rejected() {
        let dir = temp_dir();
        let profile = fixture_profile(0);
        let profile_path = write_json(&dir, "profile.json", &profile);
        let mut value = serde_json::to_value(policy(&profile)).unwrap();
        value["unknownField"] = serde_json::json!(true);
        let policy_path = write_json(&dir, "policy-v2-unknown.json", &value);
        let err = load_pipeline_from_paths(&profile_path, &policy_path)
            .expect_err("unknown policy field should fail");
        assert_eq!(err.reason, NeutralReason::InvalidPolicy);
    }

    #[test]
    fn policy_v2_fingerprint_mismatch_is_unavailable_with_mismatch_reason() {
        let dir = temp_dir();
        let profile = fixture_profile(0);
        let real_fingerprint = CalibrationProfileFingerprintV1::for_profile(&profile).unwrap();
        let mut mismatched_policy = policy(&profile);
        mismatched_policy.calibration_profile_fingerprint.digest =
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned();
        assert_ne!(
            mismatched_policy.calibration_profile_fingerprint.digest,
            real_fingerprint.digest
        );
        let profile_path = write_json(&dir, "profile.json", &profile);
        let policy_path = write_json(&dir, "policy.json", &mismatched_policy);
        let snapshot = snapshot_for_pipeline(
            load_pipeline_from_paths(&profile_path, &policy_path).expect("pipeline object"),
        );
        assert_eq!(snapshot.validity, LiveMotionState::Unavailable);
        assert_eq!(
            snapshot.neutral_reason,
            NeutralReason::CalibrationPolicyProvenanceMismatch
        );
        assert_eq!(snapshot.target_tilt.roll_rad, 0.0);
        assert_eq!(snapshot.target_tilt.pitch_rad, 0.0);
    }

    #[test]
    fn compatible_profile_and_policy_create_active_pipeline() {
        let dir = temp_dir();
        let profile = fixture_profile(0);
        let profile_path = write_json(&dir, "profile.json", &profile);
        let policy_path = write_json(&dir, "policy.json", &policy(&profile));
        let snapshot = snapshot_for_pipeline(
            load_pipeline_from_paths(&profile_path, &policy_path).expect("pipeline should load"),
        );
        assert_eq!(snapshot.validity, LiveMotionState::AwaitingSample);
        assert_eq!(snapshot.neutral_reason, NeutralReason::AwaitingFirstSample);
    }

    #[tokio::test]
    async fn invalid_config_does_not_prevent_receiver_startup() {
        let pipeline = load_pipeline_from_config(LiveTiltRuntimeConfig::new(None, None))
            .unwrap_or_else(|err| {
                LiveMotionPipeline::unavailable_with_runtime_error(err.reason, err.to_string())
            });
        let live_tilt = start_live_tilt_runtime(pipeline);
        let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
        let (sample_tx, _sample_rx) = watch::channel(None);
        let receiver = start_udp_receiver(
            crate::receiver::udp::UdpReceiverConfig {
                bind_addr: "127.0.0.1:0".parse().unwrap(),
                accepted_sample_sink: Some(live_tilt.sink.clone()),
                ..crate::receiver::udp::UdpReceiverConfig::default()
            },
            shared_state,
            sample_tx,
        )
        .await
        .expect("receiver should start with unavailable tilt pipeline");
        receiver.shutdown().await;
        live_tilt.shutdown().await;
    }
}
