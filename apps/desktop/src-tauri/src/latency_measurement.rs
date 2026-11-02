use crate::{
    calibration::provenance::CalibrationProfileFingerprintV1,
    latency_recorder::LatencyRecord,
    measurement_integrity::{evaluate_shared_integrity, GateResult, SharedIntegrityInput},
    motion_filtering::selection::TiltEstimatorPolicyV2,
    protocol::MotionSampleV1,
    transport_measurement::Distribution,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::Path,
};

pub const MEASUREMENT_PROTOCOL_VERSION: u8 = 1;
pub const LATENCY_DATASET_VERSION: u8 = 1;
pub const LATENCY_REPORT_VERSION: u8 = 1;
pub const MAX_LATENCY_DATAGRAM_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct B4bConfigurationV1 {
    pub profile_path: String,
    pub policy_path: String,
    pub calibration_profile_fingerprint: CalibrationProfileFingerprintV1,
    pub policy: TiltEstimatorPolicyV2,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LatencyPacketV1 {
    LatencyHelloV1 {
        measurement_protocol_version: u8,
        measurement_id: String,
    },
    ClockSyncRequestV1 {
        measurement_protocol_version: u8,
        measurement_id: String,
        nonce: String,
        t1_desktop_us: u64,
    },
    ClockSyncResponseV1 {
        measurement_protocol_version: u8,
        measurement_id: String,
        nonce: String,
        t1_desktop_us: u64,
        t2_android_us: u64,
        t3_android_us: u64,
    },
    LatencySampleV1 {
        measurement_protocol_version: u8,
        measurement_id: String,
        android_sensor_event_us: u64,
        android_send_preparation_us: u64,
        session_id: String,
        sequence: u32,
        sample: MotionSampleV1,
    },
}
impl LatencyPacketV1 {
    pub fn identity(&self) -> (u8, &str) {
        match self {
            Self::LatencyHelloV1 {
                measurement_protocol_version,
                measurement_id,
            }
            | Self::ClockSyncRequestV1 {
                measurement_protocol_version,
                measurement_id,
                ..
            }
            | Self::ClockSyncResponseV1 {
                measurement_protocol_version,
                measurement_id,
                ..
            }
            | Self::LatencySampleV1 {
                measurement_protocol_version,
                measurement_id,
                ..
            } => (*measurement_protocol_version, measurement_id),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        let (v, id) = self.identity();
        if v != 1 {
            return Err("unsupported_measurement_protocol_version".into());
        }
        if id.is_empty() || id.len() > 128 {
            return Err("invalid_measurement_id".into());
        }
        match self {
            Self::ClockSyncRequestV1 { nonce, .. } | Self::ClockSyncResponseV1 { nonce, .. }
                if nonce.len() < 32 || nonce.len() > 128 =>
            {
                Err("invalid_nonce".into())
            }
            Self::ClockSyncResponseV1 {
                t2_android_us,
                t3_android_us,
                ..
            } if t3_android_us < t2_android_us => Err("android_timestamp_regression".into()),
            Self::LatencySampleV1 {
                android_sensor_event_us,
                android_send_preparation_us,
                session_id,
                sequence,
                sample,
                ..
            } => {
                if android_send_preparation_us < android_sensor_event_us {
                    return Err("android_timestamp_regression".into());
                }
                if session_id != &sample.session_id || *sequence != sample.sequence {
                    return Err("sample_identity_mismatch".into());
                }
                sample
                    .validate()
                    .map_err(|e| format!("invalid_motion_sample:{e}"))
            }
            _ => Ok(()),
        }
    }
}
pub fn parse_packet(bytes: &[u8]) -> Result<LatencyPacketV1, String> {
    if bytes.len() > MAX_LATENCY_DATAGRAM_BYTES {
        return Err("oversized_datagram".into());
    }
    let p = serde_json::from_slice(bytes).map_err(|_| "invalid_packet".to_owned())?;
    LatencyPacketV1::validate(&p)?;
    Ok(p)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClockAnchorV1 {
    pub anchor_id: String,
    pub desktop_midpoint_us: f64,
    pub network_rtt_us: u64,
    pub offset_midpoint_us: f64,
    pub offset_lower_us: i128,
    pub offset_upper_us: i128,
    pub offset_uncertainty_us: f64,
    pub symmetric_one_way_estimate_us: f64,
}
pub fn clock_anchor(
    id: String,
    t1: u64,
    t2: u64,
    t3: u64,
    t4: u64,
) -> Result<ClockAnchorV1, String> {
    if t4 < t1 || t3 < t2 {
        return Err("timestamp_regression".into());
    }
    let desktop = i128::from(t4) - i128::from(t1);
    let android = i128::from(t3) - i128::from(t2);
    let rtt = desktop.checked_sub(android).ok_or("overflow")?;
    if rtt < 0 {
        return Err("negative_network_rtt".into());
    }
    let upper = i128::from(t2)
        .checked_sub(i128::from(t1))
        .ok_or("overflow")?;
    let lower = i128::from(t3)
        .checked_sub(i128::from(t4))
        .ok_or("overflow")?;
    if upper < lower {
        return Err("impossible_offset_bounds".into());
    }
    Ok(ClockAnchorV1 {
        anchor_id: id,
        desktop_midpoint_us: (t1 as f64 + t4 as f64) / 2.0,
        network_rtt_us: u64::try_from(rtt).map_err(|_| "overflow")?,
        offset_midpoint_us: (upper + lower) as f64 / 2.0,
        offset_lower_us: lower,
        offset_upper_us: upper,
        offset_uncertainty_us: (upper - lower) as f64 / 2.0,
        symmetric_one_way_estimate_us: rtt as f64 / 2.0,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClockModelV1 {
    pub offset_midpoint_us: f64,
    pub uncertainty_us: f64,
    pub before_anchor_id: String,
    pub after_anchor_id: String,
    pub max_anchor_distance_us: f64,
}
pub fn interpolate(anchors: &[ClockAnchorV1], time: f64) -> Result<ClockModelV1, String> {
    let p = anchors
        .windows(2)
        .find(|p| p[0].desktop_midpoint_us <= time && time <= p[1].desktop_midpoint_us)
        .ok_or("no_enclosing_anchors")?;
    let span = p[1].desktop_midpoint_us - p[0].desktop_midpoint_us;
    if !span.is_finite() || span <= 0.0 {
        return Err("invalid_anchor_order".into());
    }
    let w = (time - p[0].desktop_midpoint_us) / span;
    Ok(ClockModelV1 {
        offset_midpoint_us: p[0].offset_midpoint_us
            + w * (p[1].offset_midpoint_us - p[0].offset_midpoint_us),
        uncertainty_us: p[0].offset_uncertainty_us
            + w * (p[1].offset_uncertainty_us - p[0].offset_uncertainty_us),
        before_anchor_id: p[0].anchor_id.clone(),
        after_anchor_id: p[1].anchor_id.clone(),
        max_anchor_distance_us: (time - p[0].desktop_midpoint_us)
            .max(p[1].desktop_midpoint_us - time),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawSampleV1 {
    pub measurement_id: String,
    pub session_id: String,
    pub sequence: u32,
    pub android_sensor_event_us: u64,
    pub android_send_preparation_us: u64,
    pub desktop_ingress_us: u64,
    pub desktop_accepted_us: u64,
    pub desktop_processed_us: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum Estimate {
    Available {
        unit: &'static str,
        estimate: f64,
        lower_bound: f64,
        upper_bound: f64,
        uncertainty_applied: f64,
        before_anchor_id: String,
        after_anchor_id: String,
    },
    Unavailable {
        unit: &'static str,
        reason: String,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleMetrics {
    pub session_id: String,
    pub sequence: u32,
    pub android_sensor_to_send_us: u64,
    pub android_send_to_desktop_ingress_estimate_us: Estimate,
    pub desktop_ingress_to_processed_tilt_us: Option<u64>,
    pub sensor_to_desktop_ingress_estimate_us: Estimate,
    pub sensor_to_processed_tilt_estimate_us: Estimate,
    pub max_anchor_distance_us: Option<f64>,
}
fn estimate(android: u64, desktop: u64, m: &ClockModelV1) -> Estimate {
    let c = desktop as f64 - (android as f64 - m.offset_midpoint_us);
    Estimate::Available {
        unit: "microseconds",
        estimate: c,
        lower_bound: c - m.uncertainty_us,
        upper_bound: c + m.uncertainty_us,
        uncertainty_applied: m.uncertainty_us,
        before_anchor_id: m.before_anchor_id.clone(),
        after_anchor_id: m.after_anchor_id.clone(),
    }
}
pub fn metrics(s: &RawSampleV1, a: &[ClockAnchorV1]) -> SampleMetrics {
    let unavailable = |r: &str| Estimate::Unavailable {
        unit: "microseconds",
        reason: r.into(),
    };
    match interpolate(a, s.desktop_ingress_us as f64) {
        Ok(m) => SampleMetrics {
            session_id: s.session_id.clone(),
            sequence: s.sequence,
            android_sensor_to_send_us: s.android_send_preparation_us - s.android_sensor_event_us,
            android_send_to_desktop_ingress_estimate_us: estimate(
                s.android_send_preparation_us,
                s.desktop_ingress_us,
                &m,
            ),
            desktop_ingress_to_processed_tilt_us: s
                .desktop_processed_us
                .and_then(|v| v.checked_sub(s.desktop_ingress_us)),
            sensor_to_desktop_ingress_estimate_us: estimate(
                s.android_sensor_event_us,
                s.desktop_ingress_us,
                &m,
            ),
            sensor_to_processed_tilt_estimate_us: s.desktop_processed_us.map_or_else(
                || unavailable("missing_processed_event"),
                |v| estimate(s.android_sensor_event_us, v, &m),
            ),
            max_anchor_distance_us: Some(m.max_anchor_distance_us),
        },
        Err(e) => SampleMetrics {
            session_id: s.session_id.clone(),
            sequence: s.sequence,
            android_sensor_to_send_us: s.android_send_preparation_us - s.android_sensor_event_us,
            android_send_to_desktop_ingress_estimate_us: unavailable(&e),
            desktop_ingress_to_processed_tilt_us: s
                .desktop_processed_us
                .and_then(|v| v.checked_sub(s.desktop_ingress_us)),
            sensor_to_desktop_ingress_estimate_us: unavailable(&e),
            sensor_to_processed_tilt_estimate_us: unavailable(&e),
            max_anchor_distance_us: None,
        },
    }
}

#[derive(Default)]
pub struct ProbeValidator {
    expected: BTreeMap<String, (SocketAddr, String, u64)>,
    accepted: BTreeSet<String>,
}
impl ProbeValidator {
    pub fn sent(&mut self, n: String, e: SocketAddr, id: String, t1: u64) {
        self.expected.insert(n, (e, id, t1));
    }
    pub fn accept(&mut self, n: &str, e: SocketAddr, id: &str, t1: u64) -> Result<(), String> {
        let Some((ee, ei, et)) = self.expected.get(n) else {
            return Err("unknown_nonce".into());
        };
        if *ee != e {
            return Err("foreign_endpoint".into());
        }
        if ei != id {
            return Err("foreign_measurement_id".into());
        }
        if *et != t1 {
            return Err("t1_mismatch".into());
        }
        if !self.accepted.insert(n.into()) {
            return Err("duplicate_response".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Counters {
    pub datagrams_received: u64,
    pub hello_accepted: u64,
    pub duplicate_hello: u64,
    pub probes_sent: u64,
    pub probes_responded: u64,
    pub probes_rejected: u64,
    pub probes_lost: u64,
    pub invalid_packets: u64,
    pub oversized_packets: u64,
    pub foreign_packets: u64,
    pub duplicate_or_out_of_order_packets: u64,
    pub foreign_session_packets: u64,
    pub rate_limited_packets: u64,
    pub accepted_samples: u64,
    pub processed_samples: u64,
    pub runtime_dropped: u64,
    pub result_dropped: u64,
    pub recorder_dropped: u64,
    pub recorder_critical_dropped: u64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    pub id: String,
    pub passed: bool,
    pub observed: Value,
    pub expected: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyReportV1 {
    pub latency_measurement_report_version: u8,
    pub measurement_id: String,
    pub b4b_configuration: B4bConfigurationV1,
    pub requested_duration_us: u64,
    pub observed_duration_us: u64,
    pub sample_count: usize,
    pub counters: Counters,
    pub network_rtt_us: Distribution,
    pub symmetric_network_one_way_estimate_us: Distribution,
    pub synchronization_uncertainty_us: Distribution,
    pub observed_offset_us: Distribution,
    pub observed_offset_change_us: AvailableScalar,
    pub maximum_anchor_distance_us: AvailableScalar,
    pub android_sensor_to_send_us: Distribution,
    pub android_send_to_desktop_ingress_estimate_us: Distribution,
    pub desktop_ingress_to_processed_tilt_us: Distribution,
    pub sensor_to_desktop_ingress_estimate_us: Distribution,
    pub sensor_to_processed_tilt_estimate_us: Distribution,
    pub sample_metrics: Vec<SampleMetrics>,
    pub gates: Vec<Gate>,
    pub passed: bool,
    pub limitations: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum AvailableScalar {
    Available {
        unit: &'static str,
        value: f64,
    },
    Unavailable {
        unit: &'static str,
        reason: &'static str,
    },
}

#[derive(Debug)]
pub struct LoadedLatencyDataset {
    pub measurement_id: String,
    pub b4b_configuration: B4bConfigurationV1,
    pub samples: Vec<RawSampleV1>,
    pub anchors: Vec<ClockAnchorV1>,
    pub counters: Counters,
    pub observed_duration_us: u64,
    pub completed: bool,
    pub requested_duration_us: u64,
    pub summary_consistent: bool,
}

pub fn load_latency_dataset(path: &Path) -> Result<LoadedLatencyDataset, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut measurement_id = None;
    let mut hello_endpoint = None;
    let mut accepted = BTreeMap::<(String, u32), RawSampleV1>::new();
    let mut processed = BTreeMap::<(String, u32), u64>::new();
    let mut requests = BTreeMap::<String, (String, String, u64)>::new();
    let mut responses = BTreeSet::new();
    let mut anchors = Vec::new();
    let mut summary = None;
    let mut requested_duration_us = None;
    let mut b4b_configuration = None;
    let mut derived = Counters::default();
    let lines = text.lines().collect::<Vec<_>>();
    for (index, line) in text.lines().enumerate() {
        let record: LatencyRecord =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", index + 1))?;
        if index == 0 && !matches!(record, LatencyRecord::Metadata { .. }) {
            return Err("metadata_must_be_first".into());
        }
        if index + 1 == lines.len() && !matches!(record, LatencyRecord::Summary { .. }) {
            return Err("summary_must_be_last".into());
        }
        let protocol_version = match &record {
            LatencyRecord::Metadata {
                measurement_protocol_version,
                ..
            }
            | LatencyRecord::Hello {
                measurement_protocol_version,
                ..
            }
            | LatencyRecord::ClockSyncRequestSent {
                measurement_protocol_version,
                ..
            }
            | LatencyRecord::ClockSyncResponseReceived {
                measurement_protocol_version,
                ..
            }
            | LatencyRecord::SampleAccepted {
                measurement_protocol_version,
                ..
            }
            | LatencyRecord::SampleProcessed {
                measurement_protocol_version,
                ..
            }
            | LatencyRecord::Rejection {
                measurement_protocol_version,
                ..
            }
            | LatencyRecord::Summary {
                measurement_protocol_version,
                ..
            } => *measurement_protocol_version,
        };
        if protocol_version != MEASUREMENT_PROTOCOL_VERSION {
            return Err("inconsistent_measurement_protocol_version".into());
        }
        let record_id = match &record {
            LatencyRecord::Metadata { .. }
            | LatencyRecord::Rejection {
                measurement_id: None,
                ..
            } => None,
            LatencyRecord::Hello { measurement_id, .. }
            | LatencyRecord::ClockSyncRequestSent { measurement_id, .. }
            | LatencyRecord::ClockSyncResponseReceived { measurement_id, .. }
            | LatencyRecord::SampleAccepted { measurement_id, .. }
            | LatencyRecord::SampleProcessed { measurement_id, .. }
            | LatencyRecord::Summary { measurement_id, .. }
            | LatencyRecord::Rejection {
                measurement_id: Some(measurement_id),
                ..
            } => Some(measurement_id),
        };
        if record_id.is_some_and(|actual| actual.is_empty() || actual.len() > 128) {
            return Err("invalid_measurement_id".into());
        }
        if let (Some(expected), Some(actual)) = (measurement_id.as_ref(), record_id) {
            if expected != actual {
                return Err("measurement_id_mismatch".into());
            }
        }
        match record {
            LatencyRecord::Metadata {
                latency_dataset_version,
                measurement_protocol_version,
                requested_duration_us: duration,
                b4b_configuration: configuration,
            } => {
                if index != 0 || requested_duration_us.is_some() {
                    return Err("duplicate_or_misplaced_metadata".into());
                }
                if latency_dataset_version != 1 || measurement_protocol_version != 1 {
                    return Err("unsupported_version".into());
                }
                if duration == 0 {
                    return Err("invalid_requested_duration".into());
                }
                configuration
                    .calibration_profile_fingerprint
                    .validate()
                    .map_err(|error| format!("invalid_profile_fingerprint:{error}"))?;
                configuration
                    .policy
                    .validate()
                    .map_err(|error| format!("invalid_policy:{error}"))?;
                if configuration.calibration_profile_fingerprint
                    != configuration.policy.calibration_profile_fingerprint
                {
                    return Err("b4b_fingerprint_mismatch".into());
                }
                requested_duration_us = Some(duration);
                b4b_configuration = Some(configuration);
            }
            LatencyRecord::Hello {
                measurement_protocol_version,
                measurement_id: id,
                endpoint,
                ..
            } => {
                derived.datagrams_received += 1;
                if measurement_protocol_version != 1 || measurement_id.is_some() {
                    return Err("duplicate_or_invalid_hello".into());
                }
                endpoint
                    .parse::<SocketAddr>()
                    .map_err(|_| "invalid_hello_endpoint")?;
                measurement_id = Some(id);
                hello_endpoint = Some(endpoint);
                derived.hello_accepted = 1
            }
            LatencyRecord::ClockSyncRequestSent {
                measurement_id,
                nonce,
                endpoint,
                t1_desktop_us,
                ..
            } => {
                if hello_endpoint.is_none() {
                    return Err("request_before_hello".into());
                }
                if Some(&endpoint) != hello_endpoint.as_ref() {
                    return Err("request_endpoint_mismatch".into());
                }
                endpoint
                    .parse::<SocketAddr>()
                    .map_err(|_| "invalid_request_endpoint")?;
                if nonce.len() < 32 || nonce.len() > 128 {
                    return Err("invalid_nonce".into());
                }
                if requests
                    .insert(nonce, (measurement_id, endpoint, t1_desktop_us))
                    .is_some()
                {
                    return Err("duplicate_nonce".into());
                }
                derived.probes_sent += 1
            }
            LatencyRecord::ClockSyncResponseReceived {
                measurement_id,
                nonce,
                endpoint,
                t1_desktop_us,
                t2_android_us,
                t3_android_us,
                t4_desktop_us,
                ..
            } => {
                derived.datagrams_received += 1;
                if hello_endpoint.is_none() {
                    return Err("response_before_hello".into());
                }
                if Some(&endpoint) != hello_endpoint.as_ref() {
                    return Err("response_endpoint_mismatch".into());
                }
                if !responses.insert(nonce.clone()) {
                    return Err("duplicate_response_nonce".into());
                }
                let Some((rid, rep, rt1)) = requests.get(&nonce) else {
                    return Err("response_without_request".into());
                };
                if rid != &measurement_id || rep != &endpoint || *rt1 != t1_desktop_us {
                    return Err("response_identity_mismatch".into());
                }
                anchors.push(clock_anchor(
                    nonce,
                    t1_desktop_us,
                    t2_android_us,
                    t3_android_us,
                    t4_desktop_us,
                )?);
                derived.probes_responded += 1
            }
            LatencyRecord::SampleAccepted {
                measurement_id,
                session_id,
                sequence,
                sample,
                android_sensor_event_us,
                android_send_preparation_us,
                desktop_ingress_us,
                desktop_accepted_us,
                ..
            } => {
                derived.datagrams_received += 1;
                if hello_endpoint.is_none() {
                    return Err("sample_before_hello".into());
                }
                sample
                    .validate()
                    .map_err(|error| format!("invalid_accepted_sample:{error}"))?;
                if android_send_preparation_us < android_sensor_event_us
                    || desktop_accepted_us < desktop_ingress_us
                {
                    return Err("sample_timestamp_regression".into());
                }
                let key = (session_id.clone(), sequence);
                if accepted
                    .insert(
                        key,
                        RawSampleV1 {
                            measurement_id,
                            session_id: session_id.clone(),
                            sequence,
                            android_sensor_event_us,
                            android_send_preparation_us,
                            desktop_ingress_us,
                            desktop_accepted_us,
                            desktop_processed_us: None,
                        },
                    )
                    .is_some()
                {
                    return Err("duplicate_sample_accepted".into());
                }
                if sample.sequence != sequence || sample.session_id != session_id {
                    return Err("accepted_identity_mismatch".into());
                }
                derived.accepted_samples += 1
            }
            LatencyRecord::SampleProcessed {
                session_id,
                sequence,
                desktop_processed_us,
                processing_completed,
                ..
            } => {
                if measurement_id.is_none() {
                    return Err("processed_before_hello".into());
                }
                if !processing_completed {
                    return Err("processing_not_completed".into());
                }
                if processed
                    .insert((session_id, sequence), desktop_processed_us)
                    .is_some()
                {
                    return Err("duplicate_sample_processed".into());
                }
                derived.processed_samples += 1
            }
            LatencyRecord::Rejection { category, .. } => {
                let incoming = !matches!(category.as_str(), "runtime_drop" | "result_drop");
                if incoming {
                    derived.datagrams_received += 1;
                }
                match category.as_str() {
                    "oversized" => derived.oversized_packets += 1,
                    "rate_limited" => derived.rate_limited_packets += 1,
                    "foreign_endpoint" | "foreign_measurement" => derived.foreign_packets += 1,
                    "duplicate_or_out_of_order" => derived.duplicate_or_out_of_order_packets += 1,
                    "foreign_session" => derived.foreign_session_packets += 1,
                    "runtime_drop" => derived.runtime_dropped += 1,
                    "result_drop" => {
                        derived.result_dropped += 1;
                        derived.processed_samples += 1;
                    }
                    "duplicate_hello" => derived.duplicate_hello += 1,
                    "sample_before_clock_anchor" | "sample_after_window" => {}
                    _ => derived.invalid_packets += 1,
                }
            }
            LatencyRecord::Summary {
                completed,
                declared_counters,
                ..
            } => {
                if index + 1 != lines.len() || summary.is_some() {
                    return Err("duplicate_or_misplaced_summary".into());
                }
                summary = Some((
                    serde_json::from_value::<Counters>(declared_counters)
                        .map_err(|e| e.to_string())?,
                    completed,
                ))
            }
        }
    }
    let (declared, completed) = summary.ok_or("missing_summary")?;
    derived.probes_lost = derived
        .probes_sent
        .saturating_sub(derived.probes_responded + derived.probes_rejected);
    // Recorder drops cannot be reconstructed from records that, by definition, were not written.
    // The final summary is their observable source; every other counter remains independently
    // recomputed and compared below.
    derived.recorder_dropped = declared.recorder_dropped;
    derived.recorder_critical_dropped = declared.recorder_critical_dropped;
    let summary_consistent = declared == derived;
    let mut samples = accepted.into_values().collect::<Vec<_>>();
    samples.sort_by_key(|s| s.desktop_ingress_us);
    for sample in &mut samples {
        sample.desktop_processed_us =
            processed.remove(&(sample.session_id.clone(), sample.sequence));
        if sample
            .desktop_processed_us
            .is_some_and(|processed_at| processed_at < sample.desktop_accepted_us)
        {
            return Err("processed_timestamp_regression".into());
        }
    }
    if !processed.is_empty() {
        return Err("processed_without_accepted".into());
    }
    anchors.sort_by(|a, b| a.desktop_midpoint_us.total_cmp(&b.desktop_midpoint_us));
    let observed_duration_us = samples
        .last()
        .zip(samples.first())
        .map_or(0, |(last, first)| {
            last.desktop_ingress_us
                .saturating_sub(first.desktop_ingress_us)
        });
    Ok(LoadedLatencyDataset {
        measurement_id: measurement_id.ok_or("missing_hello")?,
        b4b_configuration: b4b_configuration.ok_or("missing_b4b_configuration")?,
        samples,
        anchors,
        counters: derived,
        observed_duration_us,
        completed,
        requested_duration_us: requested_duration_us.ok_or("missing_metadata")?,
        summary_consistent,
    })
}
fn dist(mut v: Vec<f64>, reason: &'static str) -> Distribution {
    if v.is_empty() {
        return Distribution::Unavailable {
            unit: "microseconds",
            count: 0,
            reason,
        };
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let q = |p: f64| {
        let h = (n - 1) as f64 * p;
        let i = h.floor() as usize;
        v[i] + (h - i as f64) * (v[h.ceil() as usize] - v[i])
    };
    Distribution::Available {
        unit: "microseconds",
        count: n,
        min: v[0],
        mean,
        population_stddev: sd,
        p50: q(0.5),
        p95: q(0.95),
        p99: q(0.99),
        max: v[n - 1],
    }
}
fn center(e: &Estimate) -> Option<f64> {
    if let Estimate::Available { estimate, .. } = e {
        Some(*estimate)
    } else {
        None
    }
}
fn gate(id: &str, ok: bool, o: Value, e: &str) -> Gate {
    Gate {
        id: id.into(),
        passed: ok,
        observed: o,
        expected: e.into(),
    }
}
pub fn report_from_analysis(analysis: LoadedLatencyDataset) -> LatencyReportV1 {
    build_report(
        analysis.measurement_id,
        analysis.b4b_configuration,
        analysis.requested_duration_us,
        analysis.observed_duration_us,
        &analysis.samples,
        &analysis.anchors,
        analysis.counters,
        analysis.completed,
        analysis.summary_consistent,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_report(
    id: String,
    b4b_configuration: B4bConfigurationV1,
    requested: u64,
    observed: u64,
    samples: &[RawSampleV1],
    anchors: &[ClockAnchorV1],
    c: Counters,
    clean: bool,
    summary_consistent: bool,
) -> LatencyReportV1 {
    let ms = samples
        .iter()
        .map(|s| metrics(s, anchors))
        .collect::<Vec<_>>();
    let sessions = samples
        .iter()
        .map(|s| &s.session_id)
        .collect::<BTreeSet<_>>();
    let transitions = samples
        .windows(2)
        .filter(|p| p[0].session_id != p[1].session_id)
        .count();
    let complete_records = ms
        .iter()
        .filter(|m| m.desktop_ingress_to_processed_tilt_us.is_some())
        .count()
        == samples.len();
    let complete = complete_records
        && c.processed_samples == c.accepted_samples
        && samples.len() as u64 + c.recorder_dropped == c.processed_samples;
    let covered = ms.iter().all(|m| m.max_anchor_distance_us.is_some());
    let essential = !samples.is_empty() && complete && covered;
    let intervals = samples
        .windows(2)
        .filter(|p| p[0].session_id == p[1].session_id)
        .collect::<Vec<_>>();
    let source_sum = intervals
        .iter()
        .map(|p| {
            p[1].android_sensor_event_us
                .saturating_sub(p[0].android_sensor_event_us)
        })
        .sum::<u64>();
    let receive_sum = intervals
        .iter()
        .map(|p| {
            p[1].desktop_ingress_us
                .saturating_sub(p[0].desktop_ingress_us)
        })
        .sum::<u64>();
    let source_rate =
        (source_sum > 0).then(|| intervals.len() as f64 * 1_000_000.0 / source_sum as f64);
    let receive_rate =
        (receive_sum > 0).then(|| intervals.len() as f64 * 1_000_000.0 / receive_sum as f64);
    let missing = intervals
        .iter()
        .map(|p| {
            u64::from(
                p[1].sequence
                    .saturating_sub(p[0].sequence)
                    .saturating_sub(1),
            )
        })
        .sum::<u64>();
    let denominator = samples.len() as u64 + missing;
    let loss = (denominator > 0).then(|| missing as f64 / denominator as f64);
    let timestamps_monotonic = samples.windows(2).all(|p| {
        p[1].desktop_ingress_us >= p[0].desktop_ingress_us
            && p[1].desktop_accepted_us >= p[0].desktop_accepted_us
    }) && samples.iter().all(|s| {
        s.desktop_accepted_us >= s.desktop_ingress_us
            && s.desktop_processed_us
                .is_none_or(|v| v >= s.desktop_accepted_us)
    });
    let mut gates = evaluate_shared_integrity(&SharedIntegrityInput {
        requested_duration_us: requested,
        observed_duration_us: observed,
        expected_sample_rate_hz: 60,
        session_count: sessions.len(),
        session_transitions: transitions,
        source_average_rate_hz: source_rate,
        receive_average_rate_hz: receive_rate,
        sequence_loss_observed: json!({
            "availability": if loss.is_some() { "available" } else { "unavailable" },
            "observedSamples": samples.len(),
            "estimatedMissingSamples": missing,
            "denominator": denominator,
            "fraction": loss,
        }),
        sequence_loss_fraction: loss,
        oversized_packets: c.oversized_packets,
        invalid_packets: c.invalid_packets,
        duplicate_or_out_of_order_packets: c.duplicate_or_out_of_order_packets,
        foreign_session_packets: c.foreign_session_packets,
        rate_limited_packets: c.rate_limited_packets,
        recorder_dropped: c.recorder_dropped,
        receiver_accepted_samples: c.accepted_samples,
        recorder_received_accepted_samples: samples.len() as u64,
        completed_cleanly: clean,
    })
    .into_iter()
    .map(|shared| Gate {
        id: shared.id.to_owned(),
        passed: shared.result == GateResult::Passed,
        observed: shared.observed,
        expected: shared.expected,
    })
    .collect::<Vec<_>>();
    gates.extend([
        gate(
            "hello.valid_unique",
            c.hello_accepted == 1 && c.duplicate_hello == 0,
            json!({"accepted":c.hello_accepted,"duplicates":c.duplicate_hello}),
            "one valid hello",
        ),
        gate(
            "sync.at_least_two_anchors",
            anchors.len() >= 2,
            json!(anchors.len()),
            ">= 2",
        ),
        gate(
            "sync.responses_paired",
            c.probes_rejected == 0,
            json!(c.probes_rejected),
            "0 rejected responses",
        ),
        gate(
            "identity.endpoint_measurement_consistent",
            c.foreign_packets == 0,
            json!(c.foreign_packets),
            "0 foreign packets",
        ),
        gate(
            "timestamps.monotonic",
            timestamps_monotonic,
            json!(timestamps_monotonic),
            "true",
        ),
        gate(
            "sync.full_coverage_no_extrapolation",
            covered,
            json!(covered),
            "true",
        ),
        gate(
            "processing.complete_correspondence",
            complete,
            json!({"samples":samples.len(),"processed":c.processed_samples}),
            "all samples processed",
        ),
        gate("runtime.configured", true, json!(true), "true"),
        gate(
            "runtime.zero_drops",
            c.runtime_dropped == 0,
            json!(c.runtime_dropped),
            "0",
        ),
        gate(
            "runtime.result_publication_zero_drops",
            c.result_dropped == 0,
            json!(c.result_dropped),
            "0",
        ),
        gate(
            "dataset.summary_consistent",
            summary_consistent,
            json!(summary_consistent),
            "true",
        ),
        gate(
            "distributions.essential_available_finite",
            essential,
            json!(essential),
            "true",
        ),
    ]);
    let passed = gates.iter().all(|g| g.passed);
    let offsets = anchors
        .iter()
        .map(|a| a.offset_midpoint_us)
        .collect::<Vec<_>>();
    let offset_change = offsets
        .last()
        .zip(offsets.first())
        .and_then(|(b, a)| {
            let value = b - a;
            value.is_finite().then_some(value)
        })
        .map_or(
            AvailableScalar::Unavailable {
                unit: "microseconds",
                reason: "fewer_than_two_finite_anchors",
            },
            |value| AvailableScalar::Available {
                unit: "microseconds",
                value,
            },
        );
    let max_distance = ms
        .iter()
        .filter_map(|m| m.max_anchor_distance_us)
        .filter(|v| v.is_finite())
        .max_by(f64::total_cmp)
        .map_or(
            AvailableScalar::Unavailable {
                unit: "microseconds",
                reason: "no_samples_with_enclosing_anchors",
            },
            |value| AvailableScalar::Available {
                unit: "microseconds",
                value,
            },
        );
    LatencyReportV1{latency_measurement_report_version:1,measurement_id:id,b4b_configuration,requested_duration_us:requested,observed_duration_us:observed,sample_count:samples.len(),counters:c,network_rtt_us:dist(anchors.iter().map(|a|a.network_rtt_us as f64).collect(),"no_valid_clock_anchors"),symmetric_network_one_way_estimate_us:dist(anchors.iter().map(|a|a.symmetric_one_way_estimate_us).collect(),"no_valid_clock_anchors"),synchronization_uncertainty_us:dist(anchors.iter().map(|a|a.offset_uncertainty_us).collect(),"no_valid_clock_anchors"),observed_offset_us:dist(offsets,"no_valid_clock_anchors"),observed_offset_change_us:offset_change,maximum_anchor_distance_us:max_distance,android_sensor_to_send_us:dist(ms.iter().map(|m|m.android_sensor_to_send_us as f64).collect(),"no_samples"),android_send_to_desktop_ingress_estimate_us:dist(ms.iter().filter_map(|m|center(&m.android_send_to_desktop_ingress_estimate_us)).collect(),"clock_model_unavailable"),desktop_ingress_to_processed_tilt_us:dist(ms.iter().filter_map(|m|m.desktop_ingress_to_processed_tilt_us.map(|v|v as f64)).collect(),"missing_processed_events"),sensor_to_desktop_ingress_estimate_us:dist(ms.iter().filter_map(|m|center(&m.sensor_to_desktop_ingress_estimate_us)).collect(),"clock_model_unavailable"),sensor_to_processed_tilt_estimate_us:dist(ms.iter().filter_map(|m|center(&m.sensor_to_processed_tilt_estimate_us)).collect(),"clock_model_or_processing_unavailable"),sample_metrics:ms,gates,passed,limitations:vec!["approximations with explicit synchronization uncertainty","RTT / 2 assumes symmetric network paths","no sensor-to-screen, sensor-to-photon, complete visual, or clinical latency measurement","no performance verdict or latency threshold"]}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);
    fn b4b_configuration() -> B4bConfigurationV1 {
        serde_json::from_value(json!({
            "profilePath": "p",
            "policyPath": "q",
            "calibrationProfileFingerprint": {
                "version": 1,
                "algorithm": "sha256",
                "digest": "0".repeat(64),
            },
            "policy": {
                "version": 2,
                "candidate": "gravity_no_additional_anchor_filter",
                "parameters": {},
                "yawAvailable": false,
                "source": "b3b_offline_selection",
                "calibrationProfileFingerprint": {
                    "version": 1,
                    "algorithm": "sha256",
                    "digest": "0".repeat(64),
                }
            }
        }))
        .unwrap()
    }
    fn valid_records() -> Vec<LatencyRecord> {
        let nonce = "n".repeat(32);
        let sample: MotionSampleV1 = serde_json::from_value(json!({"protocolVersion":1,"kind":"motion_sample","sessionId":"session-a","sequence":1,"sessionElapsedUs":1000,"linearAccelerationMps2":{"x":0.0,"y":0.0,"z":0.0},"gravityMps2":{"x":0.0,"y":0.0,"z":-9.8},"angularVelocityRadS":{"x":0.0,"y":0.0,"z":0.0}})).unwrap();
        let counters = Counters {
            datagrams_received: 3,
            hello_accepted: 1,
            probes_sent: 1,
            probes_responded: 1,
            accepted_samples: 1,
            processed_samples: 1,
            ..Counters::default()
        };
        vec![
            LatencyRecord::Metadata {
                latency_dataset_version: 1,
                measurement_protocol_version: 1,
                requested_duration_us: 60_000_000,
                b4b_configuration: b4b_configuration(),
            },
            LatencyRecord::Hello {
                measurement_protocol_version: 1,
                measurement_id: "m".into(),
                endpoint: "127.0.0.1:1".into(),
                desktop_ingress_us: 1,
            },
            LatencyRecord::ClockSyncRequestSent {
                measurement_protocol_version: 1,
                measurement_id: "m".into(),
                nonce: nonce.clone(),
                endpoint: "127.0.0.1:1".into(),
                t1_desktop_us: 100,
            },
            LatencyRecord::ClockSyncResponseReceived {
                measurement_protocol_version: 1,
                measurement_id: "m".into(),
                nonce,
                endpoint: "127.0.0.1:1".into(),
                t1_desktop_us: 100,
                t2_android_us: 1100,
                t3_android_us: 1110,
                t4_desktop_us: 120,
            },
            LatencyRecord::SampleAccepted {
                measurement_protocol_version: 1,
                measurement_id: "m".into(),
                session_id: "session-a".into(),
                sequence: 1,
                sample,
                android_sensor_event_us: 1105,
                android_send_preparation_us: 1106,
                desktop_ingress_us: 115,
                desktop_accepted_us: 116,
            },
            LatencyRecord::SampleProcessed {
                measurement_protocol_version: 1,
                measurement_id: "m".into(),
                session_id: "session-a".into(),
                sequence: 1,
                desktop_processed_us: 118,
                processing_completed: true,
                b4b_state: "valid".into(),
            },
            LatencyRecord::Summary {
                measurement_protocol_version: 1,
                measurement_id: "m".into(),
                completed: true,
                declared_counters: serde_json::to_value(counters).unwrap(),
            },
        ]
    }
    fn load_records(records: &[LatencyRecord]) -> Result<LoadedLatencyDataset, String> {
        let text = records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        load_text(&text)
    }
    fn load_text(text: &str) -> Result<LoadedLatencyDataset, String> {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "anchor-b5b-loader-{}-{id}.ndjson",
            std::process::id()
        ));
        std::fs::write(&path, text).unwrap();
        let result = load_latency_dataset(&path);
        let _ = std::fs::remove_file(path);
        result
    }
    #[test]
    fn raw_dataset_is_reconstructed_and_summary_checked() {
        let loaded = load_records(&valid_records()).unwrap();
        assert_eq!(loaded.samples.len(), 1);
        assert_eq!(loaded.anchors.len(), 1);
        assert!(loaded.summary_consistent);
        assert_eq!(loaded.samples[0].desktop_processed_us, Some(118));
    }
    #[test]
    fn loader_rejects_metadata_missing_duplicate_and_misplaced() {
        let mut r = valid_records();
        r.remove(0);
        assert!(load_records(&r).is_err());
        let mut r = valid_records();
        r.insert(1, r[0].clone());
        assert!(load_records(&r).is_err());
        let mut r = valid_records();
        r.swap(0, 1);
        assert!(load_records(&r).is_err());
    }
    #[test]
    fn loader_rejects_hello_missing_and_duplicate() {
        let mut r = valid_records();
        r.remove(1);
        assert!(load_records(&r).is_err());
        let mut r = valid_records();
        r.insert(2, r[1].clone());
        assert!(load_records(&r).is_err());
    }
    #[test]
    fn loader_rejects_summary_missing_duplicate_and_nonfinal() {
        let mut r = valid_records();
        r.pop();
        assert!(load_records(&r).is_err());
        let mut r = valid_records();
        let s = r.last().unwrap().clone();
        r.insert(r.len() - 1, s);
        assert!(load_records(&r).is_err());
        let mut r = valid_records();
        let s = r.pop().unwrap();
        r.insert(2, s);
        assert!(load_records(&r).is_err());
    }
    #[test]
    fn loader_rejects_response_without_request_and_duplicate_response() {
        let mut r = valid_records();
        r.remove(2);
        assert!(load_records(&r).is_err());
        let mut r = valid_records();
        r.insert(4, r[3].clone());
        assert!(load_records(&r).is_err());
    }
    #[test]
    fn loader_rejects_processed_without_accepted_and_duplicate_processing() {
        let mut r = valid_records();
        r.remove(4);
        assert!(load_records(&r).is_err());
        let mut r = valid_records();
        r.insert(6, r[5].clone());
        assert!(load_records(&r).is_err());
    }
    #[test]
    fn loader_marks_inconsistent_summary() {
        let mut r = valid_records();
        if let LatencyRecord::Summary {
            declared_counters, ..
        } = r.last_mut().unwrap()
        {
            *declared_counters = serde_json::to_value(Counters::default()).unwrap()
        }
        assert!(!load_records(&r).unwrap().summary_consistent);
    }
    #[test]
    fn loader_preserves_observable_recorder_drop_counters() {
        let mut r = valid_records();
        if let LatencyRecord::Summary {
            declared_counters, ..
        } = r.last_mut().unwrap()
        {
            let mut counters: Counters = serde_json::from_value(declared_counters.clone()).unwrap();
            counters.recorder_dropped = 2;
            counters.recorder_critical_dropped = 1;
            *declared_counters = serde_json::to_value(counters).unwrap();
        }
        let loaded = load_records(&r).unwrap();
        assert!(loaded.summary_consistent);
        assert_eq!(loaded.counters.recorder_dropped, 2);
        assert_eq!(loaded.counters.recorder_critical_dropped, 1);
    }
    #[test]
    fn loader_counts_samples_outside_window_without_marking_them_invalid() {
        let mut records = valid_records();
        let summary = records.pop().unwrap();
        for category in ["sample_before_clock_anchor", "sample_after_window"] {
            records.push(LatencyRecord::Rejection {
                measurement_protocol_version: 1,
                measurement_id: Some("m".into()),
                desktop_us: 200,
                category: category.into(),
                reason: "outside_nominal_window".into(),
            });
        }
        let mut summary = summary;
        if let LatencyRecord::Summary {
            declared_counters, ..
        } = &mut summary
        {
            let mut counters: Counters = serde_json::from_value(declared_counters.clone()).unwrap();
            counters.datagrams_received += 2;
            *declared_counters = serde_json::to_value(counters).unwrap();
        }
        records.push(summary);
        let loaded = load_records(&records).unwrap();
        assert!(loaded.summary_consistent);
        assert_eq!(loaded.counters.datagrams_received, 5);
        assert_eq!(loaded.counters.invalid_packets, 0);
    }
    #[test]
    fn loader_rejects_truncated_json_and_unknown_record_fields() {
        assert!(load_text("{\"recordType\":\"metadata\"").is_err());
        let mut values = valid_records()
            .iter()
            .map(|record| serde_json::to_value(record).unwrap())
            .collect::<Vec<_>>();
        values[1]["unknown"] = json!(true);
        let text = values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert!(load_text(&text).is_err());
    }
    #[test]
    fn loader_rejects_every_record_protocol_version_divergence() {
        for index in 0..valid_records().len() {
            let mut values = valid_records()
                .iter()
                .map(|record| serde_json::to_value(record).unwrap())
                .collect::<Vec<_>>();
            values[index]["measurement_protocol_version"] = json!(2);
            let text = values
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            assert_eq!(
                load_text(&text).unwrap_err(),
                "inconsistent_measurement_protocol_version",
                "record index {index}"
            );
        }
    }
    #[test]
    fn loader_rejects_every_measurement_identity_divergence() {
        for index in 2..valid_records().len() {
            let mut values = valid_records()
                .iter()
                .map(|record| serde_json::to_value(record).unwrap())
                .collect::<Vec<_>>();
            values[index]["measurement_id"] = json!("other");
            let text = values
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            assert_eq!(
                load_text(&text).unwrap_err(),
                "measurement_id_mismatch",
                "record index {index}"
            );
        }
    }
    #[test]
    fn loader_rejects_request_and_response_endpoint_divergence() {
        let mut r = valid_records();
        if let LatencyRecord::ClockSyncRequestSent { endpoint, .. } = &mut r[2] {
            *endpoint = "127.0.0.1:2".into();
        }
        assert_eq!(load_records(&r).unwrap_err(), "request_endpoint_mismatch");

        let mut r = valid_records();
        if let LatencyRecord::ClockSyncResponseReceived { endpoint, .. } = &mut r[3] {
            *endpoint = "127.0.0.1:2".into();
        }
        assert_eq!(load_records(&r).unwrap_err(), "response_endpoint_mismatch");
    }
    #[test]
    fn loader_rejects_invalid_or_mismatched_b4b_contract() {
        let mut r = valid_records();
        if let LatencyRecord::Metadata {
            b4b_configuration, ..
        } = &mut r[0]
        {
            b4b_configuration
                .policy
                .calibration_profile_fingerprint
                .digest = "1".repeat(64);
        }
        assert_eq!(load_records(&r).unwrap_err(), "b4b_fingerprint_mismatch");

        let mut r = valid_records();
        if let LatencyRecord::Metadata {
            b4b_configuration, ..
        } = &mut r[0]
        {
            b4b_configuration.calibration_profile_fingerprint.algorithm = "sha512".into();
        }
        assert!(load_records(&r)
            .unwrap_err()
            .starts_with("invalid_profile_fingerprint:"));
    }
    #[test]
    fn complete_dataset_report_is_byte_deterministic() {
        let first = serde_json::to_vec_pretty(&report_from_analysis(
            load_records(&valid_records()).unwrap(),
        ))
        .unwrap();
        let second = serde_json::to_vec_pretty(&report_from_analysis(
            load_records(&valid_records()).unwrap(),
        ))
        .unwrap();
        assert_eq!(first, second);
    }
    #[test]
    fn formulas() {
        let a = clock_anchor("a".into(), 100, 1110, 1120, 120).unwrap();
        assert_eq!(
            (a.network_rtt_us, a.offset_lower_us, a.offset_upper_us),
            (10, 1000, 1010)
        );
        assert_eq!(
            (
                a.offset_midpoint_us,
                a.offset_uncertainty_us,
                a.symmetric_one_way_estimate_us
            ),
            (1005., 5., 5.)
        );
    }
    #[test]
    fn asymmetric() {
        let a = clock_anchor("a".into(), 100, 1102, 1104, 120).unwrap();
        assert_eq!(
            (
                a.offset_lower_us,
                a.offset_upper_us,
                a.offset_uncertainty_us
            ),
            (984, 1002, 9.)
        );
    }
    #[test]
    fn invalid_math() {
        assert!(clock_anchor("x".into(), 100, 1000, 1100, 150).is_err());
        assert!(clock_anchor("x".into(), 2, 0, 1, 1).is_err());
    }
    #[test]
    fn drift_and_no_extrapolation() {
        let a = clock_anchor("a".into(), 0, 1000, 1000, 0).unwrap();
        let b = clock_anchor("b".into(), 200, 1220, 1220, 200).unwrap();
        assert_eq!(
            interpolate(&[a.clone(), b], 100.)
                .unwrap()
                .offset_midpoint_us,
            1010.
        );
        assert!(interpolate(&[a], 100.).is_err());
    }
    #[test]
    fn validator() {
        let e = "127.0.0.1:1".parse().unwrap();
        let x = "127.0.0.1:2".parse().unwrap();
        let mut v = ProbeValidator::default();
        v.sent("n".into(), e, "m".into(), 1);
        assert!(v.accept("bad", e, "m", 1).is_err());
        assert!(v.accept("n", x, "m", 1).is_err());
        assert!(v.accept("n", e, "m", 1).is_ok());
        assert!(v.accept("n", e, "m", 1).is_err());
    }
    #[test]
    fn unavailable_not_zero() {
        assert!(matches!(
            dist(vec![], "missing"),
            Distribution::Unavailable { count: 0, .. }
        ));
    }
    #[test]
    fn deterministic() {
        let r = build_report(
            "m".into(),
            b4b_configuration(),
            1,
            0,
            &[],
            &[],
            Counters::default(),
            false,
            false,
        );
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            serde_json::to_string(&r).unwrap()
        );
    }
    #[test]
    fn known_offset_converts_without_direct_clock_subtraction() {
        let anchors = vec![
            clock_anchor("a".into(), 0, 1_000, 1_000, 0).unwrap(),
            clock_anchor("b".into(), 200, 1_200, 1_200, 200).unwrap(),
        ];
        let raw = RawSampleV1 {
            measurement_id: "m".into(),
            session_id: "s".into(),
            sequence: 1,
            android_sensor_event_us: 1_080,
            android_send_preparation_us: 1_090,
            desktop_ingress_us: 100,
            desktop_accepted_us: 101,
            desktop_processed_us: Some(105),
        };
        let value = metrics(&raw, &anchors);
        assert_eq!(value.android_sensor_to_send_us, 10);
        assert!(
            matches!(value.sensor_to_desktop_ingress_estimate_us, Estimate::Available { estimate, .. } if estimate == 20.0)
        );
        assert_eq!(value.desktop_ingress_to_processed_tilt_us, Some(5));
    }

    #[test]
    fn extreme_unsigned_timestamps_do_not_overflow() {
        let anchor = clock_anchor(
            "max".into(),
            u64::MAX - 10,
            u64::MAX - 9,
            u64::MAX - 8,
            u64::MAX,
        )
        .unwrap();
        assert!(anchor.offset_midpoint_us.is_finite());
    }

    #[test]
    fn strict_auxiliary_packet_rejects_unknown_fields() {
        assert_eq!(parse_packet(br#"{"kind":"latency_hello_v1","measurement_protocol_version":1,"measurement_id":"m","unknown":true}"#).unwrap_err(), "invalid_packet");
    }
}
