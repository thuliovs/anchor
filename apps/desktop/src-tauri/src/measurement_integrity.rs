use serde::Serialize;
use serde_json::{json, Value};

pub const DURATION_SHORTFALL_TOLERANCE_US: u64 = 250_000;
pub const RATE_TOLERANCE_FRACTION: f64 = 0.05;
pub const MAX_SEQUENCE_LOSS_FRACTION: f64 = 0.001;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrityGate {
    pub id: &'static str,
    pub observed: Value,
    pub expected: String,
    pub result: GateResult,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateResult {
    Passed,
    Failed,
}

pub struct SharedIntegrityInput {
    pub requested_duration_us: u64,
    pub observed_duration_us: u64,
    pub expected_sample_rate_hz: u16,
    pub session_count: usize,
    pub session_transitions: usize,
    pub source_average_rate_hz: Option<f64>,
    pub receive_average_rate_hz: Option<f64>,
    pub sequence_loss_observed: Value,
    pub sequence_loss_fraction: Option<f64>,
    pub oversized_packets: u64,
    pub invalid_packets: u64,
    pub duplicate_or_out_of_order_packets: u64,
    pub foreign_session_packets: u64,
    pub rate_limited_packets: u64,
    pub recorder_dropped: u64,
    pub receiver_accepted_samples: u64,
    pub recorder_received_accepted_samples: u64,
    pub completed_cleanly: bool,
}

pub fn evaluate_shared_integrity(input: &SharedIntegrityInput) -> Vec<IntegrityGate> {
    let minimum_duration = input
        .requested_duration_us
        .saturating_sub(DURATION_SHORTFALL_TOLERANCE_US);
    let expected_rate = f64::from(input.expected_sample_rate_hz);
    let expected_receive = input
        .source_average_rate_hz
        .zip(input.sequence_loss_fraction)
        .map(|(value, loss)| value * (1.0 - loss));
    let mut gates = vec![
        gate(
            "duration.minimum_observed",
            json!(input.observed_duration_us),
            format!("observedDurationUs >= {minimum_duration}"),
            input.observed_duration_us >= minimum_duration,
        ),
        gate(
            "session.exactly_one",
            json!(input.session_count),
            "sessionCount == 1".to_owned(),
            input.session_count == 1,
        ),
        gate(
            "session.no_transitions",
            json!(input.session_transitions),
            "sessionTransitions == 0".to_owned(),
            input.session_transitions == 0,
        ),
        gate(
            "rate.source_expected",
            input.source_average_rate_hz.map_or(Value::Null, |value| json!(value)),
            format!(
                "finite positive source rate within ±5% of {} Hz",
                input.expected_sample_rate_hz
            ),
            input.source_average_rate_hz.is_some_and(|value| {
                value.is_finite()
                    && value > 0.0
                    && ((value - expected_rate) / expected_rate).abs()
                        <= RATE_TOLERANCE_FRACTION
            }),
        ),
        gate(
            "rate.receive_loss_coherent",
            input.receive_average_rate_hz.map_or(Value::Null, |value| json!(value)),
            "finite positive receive rate within ±5% of sourceRate * (1 - sequenceLossFraction)".to_owned(),
            input.receive_average_rate_hz.zip(expected_receive).is_some_and(
                |(observed, expected)| {
                    observed.is_finite()
                        && observed > 0.0
                        && expected.is_finite()
                        && expected > 0.0
                        && ((observed - expected) / expected).abs()
                            <= RATE_TOLERANCE_FRACTION
                },
            ),
        ),
        gate(
            "continuity.sequence_loss",
            input.sequence_loss_observed.clone(),
            format!("estimatedMissingSamples / (observedSamples + estimatedMissingSamples) <= {MAX_SEQUENCE_LOSS_FRACTION}"),
            input.sequence_loss_fraction.is_some_and(|fraction| {
                fraction.is_finite() && fraction <= MAX_SEQUENCE_LOSS_FRACTION
            }),
        ),
    ];
    for (id, value) in [
        ("receiver.oversized_zero", input.oversized_packets),
        ("receiver.invalid_zero", input.invalid_packets),
        (
            "receiver.duplicate_or_out_of_order_zero",
            input.duplicate_or_out_of_order_packets,
        ),
        (
            "receiver.foreign_session_zero",
            input.foreign_session_packets,
        ),
        ("receiver.rate_limited_zero", input.rate_limited_packets),
    ] {
        gates.push(gate(id, json!(value), "value == 0".to_owned(), value == 0));
    }
    gates.push(gate(
        "recorder.dropped_zero",
        json!(input.recorder_dropped),
        "recorderDroppedSamples == 0".to_owned(),
        input.recorder_dropped == 0,
    ));
    gates.push(gate(
        "path.accepted_samples_consistent",
        json!({
            "receiverAcceptedSamples": input.receiver_accepted_samples,
            "recorderReceivedAcceptedSamples": input.recorder_received_accepted_samples,
        }),
        "receiverAcceptedSamples == recorderReceivedAcceptedSamples".to_owned(),
        input.receiver_accepted_samples == input.recorder_received_accepted_samples,
    ));
    gates.push(gate(
        "recorder.completed_cleanly",
        json!(if input.completed_cleanly {
            "clean"
        } else {
            "interrupted"
        }),
        "completion == clean".to_owned(),
        input.completed_cleanly,
    ));
    gates
}

pub fn gate(id: &'static str, observed: Value, expected: String, passed: bool) -> IntegrityGate {
    IntegrityGate {
        id,
        observed,
        expected,
        result: if passed {
            GateResult::Passed
        } else {
            GateResult::Failed
        },
        message: if passed {
            "integrity condition satisfied".to_owned()
        } else {
            "integrity condition not satisfied".to_owned()
        },
    }
}
