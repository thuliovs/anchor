use crate::{
    calibration::provenance::CalibrationProfileFingerprintV1,
    calibration::CalibrationProfileV1,
    motion_filtering::{estimator::AngleEstimate, selection::TiltEstimatorPolicyV2},
    receiver::{AcceptedSampleEvent, SESSION_TIMEOUT},
};
use serde::Serialize;
use std::time::{Duration, Instant};

use super::{
    factory::EstimatorFactory,
    metrics::{LiveMotionMetricAccumulators, LiveMotionMetrics},
    prepared_calibration::PreparedCalibrationV1,
};

pub const MAX_CONTIGUOUS_GAP: Duration = Duration::from_millis(100);
pub const WARM_UP_SAMPLE_COUNT: usize = 2;
pub const STALE_AFTER: Duration = crate::receiver::STALE_AFTER;
pub const DISCONNECTED_AFTER: Duration = SESSION_TIMEOUT;

#[derive(Debug)]
pub enum LiveMotionPipeline {
    Unavailable(Box<UnavailablePipelineState>),
    Ready(Box<LiveMotionProcessor>),
}

#[derive(Debug)]
pub struct UnavailablePipelineState {
    reason: NeutralReason,
    fingerprint: Option<CalibrationProfileFingerprintV1>,
    policy_version: Option<u8>,
    metrics: LiveMotionMetricAccumulators,
    last_error: Option<String>,
}

impl LiveMotionPipeline {
    pub fn new(profile: &CalibrationProfileV1, policy: TiltEstimatorPolicyV2) -> Self {
        let prepared = match PreparedCalibrationV1::new(profile) {
            Ok(prepared) => prepared,
            Err(err) => {
                return Self::unavailable_with_details(
                    NeutralReason::InvalidProfile,
                    None,
                    Some(policy.version),
                    Some(err.to_string()),
                )
            }
        };
        if let Err(err) = policy.validate() {
            return Self::unavailable_with_details(
                NeutralReason::InvalidPolicy,
                Some(prepared.fingerprint().clone()),
                Some(policy.version),
                Some(err.to_string()),
            );
        }
        if &policy.calibration_profile_fingerprint != prepared.fingerprint() {
            let metrics = LiveMotionMetricAccumulators {
                provenance_mismatches: 1,
                ..LiveMotionMetricAccumulators::default()
            };
            return Self::Unavailable(Box::new(UnavailablePipelineState {
                reason: NeutralReason::CalibrationPolicyProvenanceMismatch,
                fingerprint: Some(prepared.fingerprint().clone()),
                policy_version: Some(policy.version),
                metrics,
                last_error: Some("calibration_policy_provenance_mismatch".to_owned()),
            }));
        }
        let factory = match EstimatorFactory::new(policy) {
            Ok(factory) => factory,
            Err(err) => {
                return Self::unavailable_with_details(
                    NeutralReason::InvalidPolicy,
                    Some(prepared.fingerprint().clone()),
                    Some(2),
                    Some(err.to_string()),
                )
            }
        };
        match LiveMotionProcessor::new(prepared, factory) {
            Ok(processor) => Self::Ready(Box::new(processor)),
            Err(err) => Self::unavailable_with_details(
                NeutralReason::InvalidPolicy,
                None,
                Some(2),
                Some(err),
            ),
        }
    }

    pub fn unavailable(reason: NeutralReason) -> Self {
        Self::unavailable_with_details(reason, None, None, None)
    }

    pub(crate) fn unavailable_with_runtime_error(reason: NeutralReason, error: String) -> Self {
        Self::unavailable_with_details(reason, None, None, Some(error))
    }

    fn unavailable_with_details(
        reason: NeutralReason,
        fingerprint: Option<CalibrationProfileFingerprintV1>,
        policy_version: Option<u8>,
        last_error: Option<String>,
    ) -> Self {
        Self::Unavailable(Box::new(UnavailablePipelineState {
            reason,
            fingerprint,
            policy_version,
            metrics: LiveMotionMetricAccumulators::default(),
            last_error,
        }))
    }

    pub fn process(&mut self, event: AcceptedSampleEvent) {
        if let Self::Ready(processor) = self {
            processor.process(event);
        }
    }

    pub fn snapshot(&self, now: Instant) -> LiveTiltSnapshotV1 {
        match self {
            Self::Ready(processor) => processor.snapshot(now),
            Self::Unavailable(state) => LiveTiltSnapshotV1::neutral(
                LiveMotionState::Unavailable,
                state.reason,
                None,
                None,
                None,
                None,
                state.fingerprint.clone(),
                state.policy_version,
                state.metrics.snapshot(),
                state.last_error.clone(),
            ),
        }
    }
}

pub struct LiveMotionProcessor {
    prepared: PreparedCalibrationV1,
    factory: EstimatorFactory,
    estimator: Box<dyn crate::motion_filtering::estimator::Estimator>,
    active_session_id: Option<String>,
    last_sequence: Option<u32>,
    last_session_elapsed_us: Option<u64>,
    last_event_received_at: Option<Instant>,
    last_received_at: Option<Instant>,
    last_estimate: Option<AngleEstimate>,
    state: LiveMotionState,
    neutral_reason: NeutralReason,
    warmup_samples: usize,
    last_processing_error: Option<String>,
    metrics: LiveMotionMetricAccumulators,
}

impl std::fmt::Debug for LiveMotionProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveMotionProcessor")
            .field("active_session_id", &self.active_session_id)
            .field("last_sequence", &self.last_sequence)
            .field("state", &self.state)
            .finish()
    }
}

impl LiveMotionProcessor {
    fn new(prepared: PreparedCalibrationV1, factory: EstimatorFactory) -> Result<Self, String> {
        let estimator = factory.build().map_err(|err| err.to_string())?;
        Ok(Self {
            prepared,
            factory,
            estimator,
            active_session_id: None,
            last_sequence: None,
            last_session_elapsed_us: None,
            last_event_received_at: None,
            last_received_at: None,
            last_estimate: None,
            state: LiveMotionState::AwaitingSample,
            neutral_reason: NeutralReason::AwaitingFirstSample,
            warmup_samples: 0,
            last_processing_error: None,
            metrics: LiveMotionMetricAccumulators::default(),
        })
    }

    pub fn process(&mut self, event: AcceptedSampleEvent) {
        self.metrics.processed_events = self.metrics.processed_events.saturating_add(1);
        self.last_processing_error = None;
        self.last_event_received_at = Some(event.received_at);

        if self
            .active_session_id
            .as_deref()
            .is_some_and(|session| session != event.sample.session_id)
        {
            self.metrics.session_resets = self.metrics.session_resets.saturating_add(1);
            self.reinitialize_with_event(event, NeutralReason::SessionChanged);
            return;
        }

        let Some(previous_sequence) = self.last_sequence else {
            self.reinitialize_with_event(event, NeutralReason::WarmUp);
            return;
        };

        if event.sample.sequence <= previous_sequence {
            self.metrics.non_monotonic_sequence_events =
                self.metrics.non_monotonic_sequence_events.saturating_add(1);
            self.invalidate_continuity(
                NeutralReason::SequenceNonMonotonic,
                "sequence_non_monotonic".to_owned(),
            );
            return;
        }

        let sequence_delta = event.sample.sequence - previous_sequence;
        if sequence_delta > 1 {
            self.metrics.sequence_gap_events = self.metrics.sequence_gap_events.saturating_add(1);
            self.metrics.estimated_missing_samples = self
                .metrics
                .estimated_missing_samples
                .saturating_add(u64::from(sequence_delta - 1));
        }

        let Some(previous_elapsed) = self.last_session_elapsed_us else {
            self.reinitialize_with_event(event, NeutralReason::WarmUp);
            return;
        };
        let Some(source_delta_us) = event
            .sample
            .session_elapsed_us
            .checked_sub(previous_elapsed)
            .filter(|delta| *delta > 0)
        else {
            self.metrics.invalid_dt_resets = self.metrics.invalid_dt_resets.saturating_add(1);
            self.reinitialize_with_event(event, NeutralReason::InvalidDt);
            return;
        };
        let Some(previous_received_at) = self.last_received_at else {
            self.reinitialize_with_event(event, NeutralReason::WarmUp);
            return;
        };
        let Some(receive_delta) = event
            .received_at
            .checked_duration_since(previous_received_at)
        else {
            self.metrics.invalid_dt_resets = self.metrics.invalid_dt_resets.saturating_add(1);
            self.reinitialize_with_event(event, NeutralReason::InvalidDt);
            return;
        };
        if receive_delta.is_zero() {
            self.metrics.invalid_dt_resets = self.metrics.invalid_dt_resets.saturating_add(1);
            self.reinitialize_with_event(event, NeutralReason::InvalidDt);
            return;
        }

        let source_delta = Duration::from_micros(source_delta_us);
        if source_delta > MAX_CONTIGUOUS_GAP || receive_delta > MAX_CONTIGUOUS_GAP {
            self.metrics.gap_resets = self.metrics.gap_resets.saturating_add(1);
            self.reinitialize_with_event(event, NeutralReason::TemporalGap);
            return;
        }

        self.record_temporal_metrics(source_delta_us, receive_delta);
        self.update_continuous(event, source_delta_us);
    }

    pub fn snapshot(&self, now: Instant) -> LiveTiltSnapshotV1 {
        let age = self
            .last_event_received_at
            .map(|last| match now.checked_duration_since(last) {
                Some(value) => value,
                None => Duration::ZERO,
            });
        let (validity, reason) = match age {
            None => (
                LiveMotionState::AwaitingSample,
                NeutralReason::AwaitingFirstSample,
            ),
            Some(value) if value > DISCONNECTED_AFTER => {
                (LiveMotionState::Disconnected, NeutralReason::Disconnected)
            }
            Some(value) if value > STALE_AFTER => (LiveMotionState::Stale, NeutralReason::Stale),
            Some(_) if self.state == LiveMotionState::Valid => {
                (LiveMotionState::Valid, NeutralReason::None)
            }
            Some(_) => (self.state, self.neutral_reason),
        };

        LiveTiltSnapshotV1::neutral(
            validity,
            reason,
            self.last_estimate.clone(),
            self.active_session_id.clone(),
            self.last_sequence,
            age.map(duration_millis_u64),
            Some(self.prepared.fingerprint().clone()),
            Some(self.factory.policy_version()),
            self.metrics.snapshot(),
            self.last_processing_error.clone(),
        )
    }

    fn record_temporal_metrics(&mut self, source_delta_us: u64, receive_delta: Duration) {
        let receive_delta_us = duration_micros_f64(receive_delta);
        let source_delta = source_delta_us as f64;
        let variation = receive_delta_us - source_delta;
        self.metrics.source_interval_us.push(source_delta);
        self.metrics.receive_interarrival_us.push(receive_delta_us);
        self.metrics.receive_minus_source_delta_us.push(variation);
        self.metrics.max_abs_receive_minus_source_delta_us = self
            .metrics
            .max_abs_receive_minus_source_delta_us
            .max(variation.abs());
    }

    fn update_continuous(&mut self, event: AcceptedSampleEvent, source_delta_us: u64) {
        let calibrated = match self.prepared.calibrate_sample(&event.sample) {
            Ok(sample) => sample,
            Err(err) => {
                self.metrics.calibration_errors = self.metrics.calibration_errors.saturating_add(1);
                self.invalidate_continuity(NeutralReason::CalibrationError, err.to_string());
                return;
            }
        };
        let dt_seconds = source_delta_us as f64 / 1_000_000.0;
        if !dt_seconds.is_finite() || dt_seconds <= 0.0 {
            self.metrics.invalid_dt_resets = self.metrics.invalid_dt_resets.saturating_add(1);
            self.reinitialize_with_event(event, NeutralReason::InvalidDt);
            return;
        }
        let estimate = match self.estimator.update(&calibrated, dt_seconds) {
            Ok(estimate) if estimate_is_safe(&estimate) => estimate,
            Ok(_) => {
                self.metrics.estimator_errors = self.metrics.estimator_errors.saturating_add(1);
                self.invalidate_continuity(
                    NeutralReason::EstimatorError,
                    "estimator produced non-finite output".to_owned(),
                );
                return;
            }
            Err(err) => {
                self.metrics.estimator_errors = self.metrics.estimator_errors.saturating_add(1);
                self.invalidate_continuity(NeutralReason::EstimatorError, err.to_string());
                return;
            }
        };
        self.last_estimate = Some(estimate);
        self.active_session_id = Some(event.sample.session_id.clone());
        self.last_sequence = Some(event.sample.sequence);
        self.last_session_elapsed_us = Some(event.sample.session_elapsed_us);
        self.last_received_at = Some(event.received_at);
        self.warmup_samples = self.warmup_samples.saturating_add(1);
        if self.warmup_samples >= WARM_UP_SAMPLE_COUNT {
            self.state = LiveMotionState::Valid;
            self.neutral_reason = NeutralReason::None;
            self.metrics.valid_outputs = self.metrics.valid_outputs.saturating_add(1);
        } else {
            self.state = LiveMotionState::WarmingUp;
            self.neutral_reason = NeutralReason::WarmUp;
        }
    }

    fn reinitialize_with_event(&mut self, event: AcceptedSampleEvent, reason: NeutralReason) {
        self.estimator = match self.factory.build() {
            Ok(estimator) => estimator,
            Err(err) => {
                self.state = LiveMotionState::Invalid;
                self.neutral_reason = NeutralReason::InvalidPolicy;
                self.last_processing_error = Some(err.to_string());
                return;
            }
        };
        let calibrated = match self.prepared.calibrate_sample(&event.sample) {
            Ok(sample) => sample,
            Err(err) => {
                self.metrics.calibration_errors = self.metrics.calibration_errors.saturating_add(1);
                self.invalidate_continuity(NeutralReason::CalibrationError, err.to_string());
                return;
            }
        };
        let estimate = match self.estimator.initialize(&calibrated) {
            Ok(estimate) if estimate_is_safe(&estimate) => estimate,
            Ok(_) => {
                self.metrics.estimator_errors = self.metrics.estimator_errors.saturating_add(1);
                self.invalidate_continuity(
                    NeutralReason::EstimatorError,
                    "estimator produced non-finite output".to_owned(),
                );
                return;
            }
            Err(err) => {
                self.metrics.estimator_errors = self.metrics.estimator_errors.saturating_add(1);
                self.invalidate_continuity(NeutralReason::EstimatorError, err.to_string());
                return;
            }
        };
        self.active_session_id = Some(event.sample.session_id.clone());
        self.last_sequence = Some(event.sample.sequence);
        self.last_session_elapsed_us = Some(event.sample.session_elapsed_us);
        self.last_event_received_at = Some(event.received_at);
        self.last_received_at = Some(event.received_at);
        self.last_estimate = Some(estimate);
        self.warmup_samples = 1;
        self.state = LiveMotionState::WarmingUp;
        self.neutral_reason = match reason {
            NeutralReason::InvalidDt
            | NeutralReason::SessionChanged
            | NeutralReason::TemporalGap => reason,
            _ => NeutralReason::WarmUp,
        };
    }

    fn invalidate_continuity(&mut self, reason: NeutralReason, error: String) {
        self.estimator = match self.factory.build() {
            Ok(estimator) => estimator,
            Err(err) => {
                self.last_processing_error = Some(err.to_string());
                self.state = LiveMotionState::Invalid;
                self.neutral_reason = NeutralReason::InvalidPolicy;
                self.last_sequence = None;
                self.last_session_elapsed_us = None;
                self.last_received_at = None;
                self.warmup_samples = 0;
                return;
            }
        };
        self.state = LiveMotionState::Invalid;
        self.neutral_reason = reason;
        self.last_processing_error = Some(error);
        self.last_sequence = None;
        self.last_session_elapsed_us = None;
        self.last_received_at = None;
        self.warmup_samples = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveMotionState {
    Unavailable,
    AwaitingSample,
    WarmingUp,
    Valid,
    Invalid,
    Stale,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NeutralReason {
    None,
    MissingConfiguration,
    InvalidProfile,
    InvalidPolicy,
    CalibrationPolicyProvenanceMismatch,
    AwaitingFirstSample,
    WarmUp,
    SessionChanged,
    TemporalGap,
    InvalidDt,
    SequenceNonMonotonic,
    CalibrationError,
    EstimatorError,
    Stale,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tilt2 {
    pub roll_rad: f64,
    pub pitch_rad: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTiltSnapshotV1 {
    pub snapshot_version: u8,
    pub validity: LiveMotionState,
    pub neutral_reason: NeutralReason,
    pub target_tilt: Tilt2,
    pub last_estimate: Option<AngleEstimate>,
    pub active_session_id: Option<String>,
    pub last_sequence: Option<u32>,
    pub last_sample_age_ms: Option<u64>,
    pub yaw_available: bool,
    pub calibration_profile_fingerprint: Option<CalibrationProfileFingerprintV1>,
    pub policy_version: Option<u8>,
    pub metrics: LiveMotionMetrics,
    pub last_processing_error: Option<String>,
}

impl LiveTiltSnapshotV1 {
    #[allow(clippy::too_many_arguments)]
    fn neutral(
        validity: LiveMotionState,
        neutral_reason: NeutralReason,
        last_estimate: Option<AngleEstimate>,
        active_session_id: Option<String>,
        last_sequence: Option<u32>,
        last_sample_age_ms: Option<u64>,
        calibration_profile_fingerprint: Option<CalibrationProfileFingerprintV1>,
        policy_version: Option<u8>,
        metrics: LiveMotionMetrics,
        last_processing_error: Option<String>,
    ) -> Self {
        let target_tilt = if validity == LiveMotionState::Valid {
            last_estimate
                .as_ref()
                .filter(|estimate| estimate.roll_rad.is_finite() && estimate.pitch_rad.is_finite())
                .map_or_else(zero_tilt, |estimate| Tilt2 {
                    roll_rad: estimate.roll_rad,
                    pitch_rad: estimate.pitch_rad,
                })
        } else {
            zero_tilt()
        };
        Self {
            snapshot_version: 1,
            validity,
            neutral_reason,
            target_tilt,
            last_estimate,
            active_session_id,
            last_sequence,
            last_sample_age_ms,
            yaw_available: false,
            calibration_profile_fingerprint,
            policy_version,
            metrics,
            last_processing_error,
        }
    }
}

fn estimate_is_safe(estimate: &AngleEstimate) -> bool {
    !estimate.yaw_available
        && estimate.roll_rad.is_finite()
        && estimate.pitch_rad.is_finite()
        && estimate.gravity_direction.x.is_finite()
        && estimate.gravity_direction.y.is_finite()
        && estimate.gravity_direction.z.is_finite()
}

fn zero_tilt() -> Tilt2 {
    Tilt2 {
        roll_rad: 0.0,
        pitch_rad: 0.0,
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn duration_micros_f64(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        motion_filtering::{
            selection::{PolicyCandidate, POLICY_SOURCE},
            synthetic::synthetic_suite,
        },
        protocol::Vector3,
    };
    use std::{collections::BTreeMap, net::SocketAddr};

    fn fixture_profile() -> CalibrationProfileV1 {
        synthetic_suite().unwrap().remove(0).calibration_profile
    }

    fn policy(profile: &CalibrationProfileV1) -> TiltEstimatorPolicyV2 {
        policy_for_candidate(
            profile,
            PolicyCandidate::GravityNoAdditionalAnchorFilter,
            BTreeMap::new(),
        )
    }

    fn policy_for_candidate(
        profile: &CalibrationProfileV1,
        candidate: PolicyCandidate,
        parameters: BTreeMap<String, f64>,
    ) -> TiltEstimatorPolicyV2 {
        TiltEstimatorPolicyV2 {
            version: 2,
            candidate,
            parameters,
            yaw_available: false,
            source: POLICY_SOURCE.to_owned(),
            calibration_profile_fingerprint: CalibrationProfileFingerprintV1::for_profile(profile)
                .unwrap(),
        }
    }

    fn event(seq: u32, elapsed: u64, at: Instant, session: &str) -> AcceptedSampleEvent {
        let mut sample = synthetic_suite().unwrap()[0].raw_samples[0].clone();
        sample.sequence = seq;
        sample.session_elapsed_us = elapsed;
        sample.session_id = session.to_owned();
        AcceptedSampleEvent {
            sample,
            sender: "127.0.0.1:1234".parse::<SocketAddr>().unwrap(),
            received_at: at,
        }
    }

    fn ready() -> (LiveMotionPipeline, Instant) {
        let profile = fixture_profile();
        let pipeline = LiveMotionPipeline::new(&profile, policy(&profile));
        (pipeline, Instant::now())
    }

    fn ready_with_policy(policy: TiltEstimatorPolicyV2) -> (LiveMotionPipeline, Instant) {
        let profile = fixture_profile();
        (LiveMotionPipeline::new(&profile, policy), Instant::now())
    }

    fn run_to_valid(pipeline: &mut LiveMotionPipeline, now: Instant) {
        pipeline.process(event(1, 0, now, "a"));
        pipeline.process(event(2, 16_667, now + Duration::from_micros(16_667), "a"));
    }

    fn assert_send<T: Send>() {}

    #[test]
    fn awaiting_first_then_warmup_then_valid() {
        let (mut pipeline, now) = ready();
        let awaiting = pipeline.snapshot(now);
        assert_eq!(awaiting.validity, LiveMotionState::AwaitingSample);
        assert_eq!(awaiting.last_sample_age_ms, None);
        pipeline.process(event(1, 0, now, "a"));
        let first = pipeline.snapshot(now);
        assert_eq!(first.validity, LiveMotionState::WarmingUp);
        assert_eq!(first.target_tilt, zero_tilt());
        assert!(first.last_estimate.is_some());
        assert_eq!(first.last_sample_age_ms, Some(0));
        pipeline.process(event(2, 16_667, now + Duration::from_micros(16_667), "a"));
        let second = pipeline.snapshot(now + Duration::from_micros(16_667));
        assert_eq!(second.validity, LiveMotionState::Valid);
        assert_eq!(second.metrics.valid_outputs, 1);
    }

    #[test]
    fn live_motion_pipeline_is_send() {
        assert_send::<LiveMotionPipeline>();
    }

    #[test]
    fn public_pipeline_does_not_allow_active_processor_with_mismatched_policy() {
        let profile = fixture_profile();
        let mut bad_policy = policy(&profile);
        bad_policy.calibration_profile_fingerprint.digest = "1".repeat(64);
        let pipeline = LiveMotionPipeline::new(&profile, bad_policy);
        assert!(matches!(pipeline, LiveMotionPipeline::Unavailable(_)));
        assert_eq!(
            pipeline.snapshot(Instant::now()).validity,
            LiveMotionState::Unavailable
        );
    }

    #[test]
    fn policy_v1_cannot_activate_live_pipeline() {
        let value = serde_json::json!({
            "version": 1,
            "candidate": "gravity_no_additional_anchor_filter",
            "parameters": {},
            "yawAvailable": false,
            "source": "b3b_offline_selection"
        });
        assert!(serde_json::from_value::<TiltEstimatorPolicyV2>(value).is_err());
        let pipeline = LiveMotionPipeline::unavailable(NeutralReason::MissingConfiguration);
        let snap = pipeline.snapshot(Instant::now());
        assert_eq!(snap.validity, LiveMotionState::Unavailable);
        assert_eq!(snap.target_tilt, zero_tilt());
        assert_eq!(snap.last_sample_age_ms, None);
    }

    #[test]
    fn pipeline_runs_all_three_estimators_to_valid() {
        let profile = fixture_profile();
        let cases = [
            policy(&profile),
            {
                let mut parameters = BTreeMap::new();
                parameters.insert("tauMs".to_owned(), 25.0);
                policy_for_candidate(&profile, PolicyCandidate::LowPassGravity, parameters)
            },
            {
                let mut parameters = BTreeMap::new();
                parameters.insert("correctionTauMs".to_owned(), 50.0);
                policy_for_candidate(
                    &profile,
                    PolicyCandidate::ComplementaryGravityGyro,
                    parameters,
                )
            },
        ];
        for policy in cases {
            let (mut pipeline, now) = ready_with_policy(policy);
            run_to_valid(&mut pipeline, now);
            assert_eq!(
                pipeline
                    .snapshot(now + Duration::from_micros(16_667))
                    .validity,
                LiveMotionState::Valid
            );
        }
    }

    #[test]
    fn stale_and_disconnected_thresholds_are_strictly_above_limits() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(1, 0, now, "a"));
        pipeline.process(event(2, 16_667, now + Duration::from_micros(16_667), "a"));
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(16_667) + STALE_AFTER)
                .validity,
            LiveMotionState::Valid
        );
        assert_eq!(
            pipeline
                .snapshot(
                    now + Duration::from_micros(16_667) + STALE_AFTER + Duration::from_nanos(1)
                )
                .validity,
            LiveMotionState::Stale
        );
        let stale = pipeline
            .snapshot(now + Duration::from_micros(16_667) + STALE_AFTER + Duration::from_nanos(1));
        assert!(stale.last_estimate.is_some());
        assert_eq!(stale.target_tilt, zero_tilt());
        assert_eq!(stale.last_sample_age_ms, Some(250));
        assert!(!stale.yaw_available);
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(16_667) + DISCONNECTED_AFTER)
                .validity,
            LiveMotionState::Stale
        );
        assert_eq!(
            pipeline
                .snapshot(
                    now + Duration::from_micros(16_667)
                        + DISCONNECTED_AFTER
                        + Duration::from_nanos(1)
                )
                .validity,
            LiveMotionState::Disconnected
        );
        let disconnected = pipeline.snapshot(
            now + Duration::from_micros(16_667) + DISCONNECTED_AFTER + Duration::from_nanos(1),
        );
        assert!(disconnected.last_estimate.is_some());
        assert_eq!(disconnected.target_tilt, zero_tilt());
        assert!(disconnected.last_sample_age_ms.is_some());
        assert!(!disconnected.yaw_available);
    }

    #[test]
    fn exactly_100ms_is_continuous_above_100ms_resets() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(1, 0, now, "a"));
        pipeline.process(event(2, 100_000, now + MAX_CONTIGUOUS_GAP, "a"));
        assert_eq!(
            pipeline.snapshot(now + MAX_CONTIGUOUS_GAP).validity,
            LiveMotionState::Valid
        );
        pipeline.process(event(
            3,
            200_001,
            now + MAX_CONTIGUOUS_GAP + MAX_CONTIGUOUS_GAP + Duration::from_micros(1),
            "a",
        ));
        let snap = pipeline.snapshot(now + Duration::from_millis(201));
        assert_eq!(snap.validity, LiveMotionState::WarmingUp);
        assert_eq!(snap.metrics.gap_resets, 1);
    }

    #[test]
    fn source_and_receive_dt_invalid_or_gap_reset_to_warmup() {
        for (elapsed, receive_offset_us, reason) in [
            (10, 16_667_i64, NeutralReason::InvalidDt),
            (1, 16_667_i64, NeutralReason::InvalidDt),
            (16_667, 0_i64, NeutralReason::InvalidDt),
            (16_667, -1_i64, NeutralReason::InvalidDt),
            (16_667, 101_000_i64, NeutralReason::TemporalGap),
            (100_011, 100_000_i64, NeutralReason::TemporalGap),
        ] {
            let (mut pipeline, now) = ready();
            pipeline.process(event(1, 10, now, "a"));
            let received_at = if receive_offset_us < 0 {
                now.checked_sub(Duration::from_micros(receive_offset_us.unsigned_abs()))
                    .unwrap_or(now)
            } else {
                now + Duration::from_micros(receive_offset_us as u64)
            };
            pipeline.process(event(2, elapsed, received_at, "a"));
            let snap = pipeline.snapshot(received_at);
            assert_eq!(snap.validity, LiveMotionState::WarmingUp);
            assert_eq!(snap.neutral_reason, reason);
            assert_eq!(snap.target_tilt, zero_tilt());
        }
    }

    #[test]
    fn interarrival_and_source_exactly_100ms_are_continuous() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(1, 0, now, "a"));
        pipeline.process(event(2, 100_000, now + Duration::from_millis(100), "a"));
        assert_eq!(
            pipeline.snapshot(now + Duration::from_millis(100)).validity,
            LiveMotionState::Valid
        );
    }

    #[test]
    fn sequence_gap_counts_missing_samples_without_reset() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(1, 0, now, "a"));
        pipeline.process(event(3, 16_667, now + Duration::from_micros(16_667), "a"));
        let snap = pipeline.snapshot(now + Duration::from_micros(16_667));
        assert_eq!(snap.validity, LiveMotionState::Valid);
        assert_eq!(snap.metrics.sequence_gap_events, 1);
        assert_eq!(snap.metrics.estimated_missing_samples, 1);
    }

    #[test]
    fn multiple_sequence_gap_counts_missing_samples_without_reset() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(1, 0, now, "a"));
        pipeline.process(event(7, 16_667, now + Duration::from_micros(16_667), "a"));
        let snap = pipeline.snapshot(now + Duration::from_micros(16_667));
        assert_eq!(snap.validity, LiveMotionState::Valid);
        assert_eq!(snap.metrics.sequence_gap_events, 1);
        assert_eq!(snap.metrics.estimated_missing_samples, 5);
    }

    #[test]
    fn duplicate_sequence_fails_closed() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(1, 0, now, "a"));
        pipeline.process(event(1, 16_667, now + Duration::from_micros(16_667), "a"));
        let snap = pipeline.snapshot(now + Duration::from_micros(16_667));
        assert_eq!(snap.validity, LiveMotionState::Invalid);
        assert_eq!(snap.neutral_reason, NeutralReason::SequenceNonMonotonic);
        assert_eq!(snap.target_tilt, zero_tilt());
    }

    #[test]
    fn regressions_and_rollover_are_not_supported() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(u32::MAX - 1, 0, now, "a"));
        pipeline.process(event(
            u32::MAX,
            16_667,
            now + Duration::from_micros(16_667),
            "a",
        ));
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(16_667))
                .validity,
            LiveMotionState::Valid
        );
        pipeline.process(event(0, 33_334, now + Duration::from_micros(33_334), "a"));
        let snap = pipeline.snapshot(now + Duration::from_micros(33_334));
        assert_eq!(snap.validity, LiveMotionState::Invalid);
        assert_eq!(snap.neutral_reason, NeutralReason::SequenceNonMonotonic);
        assert_eq!(snap.metrics.non_monotonic_sequence_events, 1);
    }

    #[test]
    fn invalid_sequence_requires_new_warmup_before_valid() {
        let (mut pipeline, now) = ready();
        run_to_valid(&mut pipeline, now);
        pipeline.process(event(2, 33_334, now + Duration::from_micros(33_334), "a"));
        let invalid = pipeline.snapshot(now + Duration::from_micros(33_334));
        assert_eq!(invalid.validity, LiveMotionState::Invalid);
        assert_eq!(invalid.target_tilt, zero_tilt());
        assert!(invalid.last_estimate.is_some());

        pipeline.process(event(3, 50_001, now + Duration::from_micros(50_001), "a"));
        let warming = pipeline.snapshot(now + Duration::from_micros(50_001));
        assert_eq!(warming.validity, LiveMotionState::WarmingUp);
        assert_eq!(warming.target_tilt, zero_tilt());

        pipeline.process(event(4, 66_668, now + Duration::from_micros(66_668), "a"));
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(66_668))
                .validity,
            LiveMotionState::Valid
        );
    }

    #[test]
    fn estimator_update_error_requires_new_warmup_before_valid() {
        let (mut pipeline, now) = ready();
        run_to_valid(&mut pipeline, now);
        let mut bad = event(3, 33_334, now + Duration::from_micros(33_334), "a");
        bad.sample.gravity_mps2 = Vector3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        pipeline.process(bad);
        let invalid = pipeline.snapshot(now + Duration::from_micros(33_334));
        assert_eq!(invalid.validity, LiveMotionState::Invalid);
        assert_eq!(invalid.neutral_reason, NeutralReason::EstimatorError);
        assert_eq!(invalid.target_tilt, zero_tilt());
        assert!(invalid.last_estimate.is_some());
        assert_eq!(invalid.last_sample_age_ms, Some(0));

        pipeline.process(event(4, 50_001, now + Duration::from_micros(50_001), "a"));
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(50_001))
                .validity,
            LiveMotionState::WarmingUp
        );
        pipeline.process(event(5, 66_668, now + Duration::from_micros(66_668), "a"));
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(66_668))
                .validity,
            LiveMotionState::Valid
        );
    }

    #[test]
    fn calibration_error_requires_new_warmup_before_valid() {
        let (mut pipeline, now) = ready();
        run_to_valid(&mut pipeline, now);
        let mut bad = event(3, 33_334, now + Duration::from_micros(33_334), "a");
        bad.sample.linear_acceleration_mps2.x = f64::NAN;
        pipeline.process(bad);
        let invalid = pipeline.snapshot(now + Duration::from_micros(33_334));
        assert_eq!(invalid.validity, LiveMotionState::Invalid);
        assert_eq!(invalid.neutral_reason, NeutralReason::CalibrationError);

        pipeline.process(event(4, 50_001, now + Duration::from_micros(50_001), "a"));
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(50_001))
                .validity,
            LiveMotionState::WarmingUp
        );
        pipeline.process(event(5, 66_668, now + Duration::from_micros(66_668), "a"));
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(66_668))
                .validity,
            LiveMotionState::Valid
        );
    }

    #[test]
    fn estimator_initialize_error_stays_invalid_until_usable_first_warmup_sample() {
        let (mut pipeline, now) = ready();
        let mut bad = event(1, 0, now, "a");
        bad.sample.gravity_mps2 = Vector3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        pipeline.process(bad);
        let invalid = pipeline.snapshot(now);
        assert_eq!(invalid.validity, LiveMotionState::Invalid);
        assert_eq!(invalid.neutral_reason, NeutralReason::EstimatorError);
        assert_eq!(invalid.last_sample_age_ms, Some(0));
        assert_eq!(invalid.target_tilt, zero_tilt());
        pipeline.process(event(2, 16_667, now + Duration::from_micros(16_667), "a"));
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(16_667))
                .validity,
            LiveMotionState::WarmingUp
        );
        pipeline.process(event(3, 33_334, now + Duration::from_micros(33_334), "a"));
        assert_eq!(
            pipeline
                .snapshot(now + Duration::from_micros(33_334))
                .validity,
            LiveMotionState::Valid
        );
    }

    #[test]
    fn provenance_mismatch_is_unavailable() {
        let profile = fixture_profile();
        let mut bad_policy = policy(&profile);
        bad_policy.calibration_profile_fingerprint.digest = "1".repeat(64);
        let pipeline = LiveMotionPipeline::new(&profile, bad_policy);
        let snap = pipeline.snapshot(Instant::now());
        assert_eq!(snap.validity, LiveMotionState::Unavailable);
        assert_eq!(
            snap.neutral_reason,
            NeutralReason::CalibrationPolicyProvenanceMismatch
        );
        assert_eq!(snap.metrics.provenance_mismatches, 1);
        assert_eq!(snap.last_sample_age_ms, None);
        assert_eq!(snap.target_tilt, zero_tilt());
    }

    #[test]
    fn snapshot_serializes_camel_case_and_no_nan() {
        let (pipeline, now) = ready();
        let json = serde_json::to_string(&pipeline.snapshot(now)).unwrap();
        assert!(json.contains("snapshotVersion"));
        assert!(json.contains("targetTilt"));
        assert!(!json.contains("NaN"));
        assert!(!json.contains("Infinity"));
    }

    #[test]
    fn snapshot_now_before_last_sample_does_not_panic() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(1, 0, now + Duration::from_secs(1), "a"));
        assert_eq!(pipeline.snapshot(now).last_sample_age_ms, Some(0));
    }

    #[test]
    fn session_change_restarts_warmup() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(1, 0, now, "a"));
        pipeline.process(event(2, 16_667, now + Duration::from_micros(16_667), "a"));
        pipeline.process(event(1, 0, now + Duration::from_micros(30_000), "b"));
        let snap = pipeline.snapshot(now + Duration::from_micros(30_000));
        assert_eq!(snap.validity, LiveMotionState::WarmingUp);
        assert_eq!(snap.metrics.session_resets, 1);
    }

    #[test]
    fn temporal_metrics_have_known_values() {
        let (mut pipeline, now) = ready();
        pipeline.process(event(1, 0, now, "a"));
        pipeline.process(event(2, 10_000, now + Duration::from_micros(12_000), "a"));
        pipeline.process(event(3, 30_000, now + Duration::from_micros(35_000), "a"));
        let metrics = pipeline
            .snapshot(now + Duration::from_micros(35_000))
            .metrics;
        assert_eq!(metrics.source_interval_us.count, 2);
        assert_eq!(metrics.receive_interarrival_us.count, 2);
        assert_eq!(metrics.receive_minus_source_delta_us.count, 2);
        assert!((metrics.source_interval_us.mean - 15_000.0).abs() < 1e-9);
        assert!((metrics.source_interval_us.population_stddev - 5_000.0).abs() < 1e-9);
        assert_eq!(metrics.source_interval_us.min, 10_000.0);
        assert_eq!(metrics.source_interval_us.max, 20_000.0);
        assert_eq!(metrics.receive_interarrival_us.min, 12_000.0);
        assert_eq!(metrics.receive_interarrival_us.max, 23_000.0);
        assert_eq!(metrics.receive_minus_source_delta_us.min, 2_000.0);
        assert_eq!(metrics.receive_minus_source_delta_us.max, 3_000.0);
        assert_eq!(metrics.max_abs_receive_minus_source_delta_us, 3_000.0);
    }
}
