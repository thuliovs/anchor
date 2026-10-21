use crate::{
    dataset::{DatasetAnalysis, DatasetSummaryRecord, IntervalStats, SignedIntervalStats},
    receiver::ReceiverMetrics,
};
use serde::Serialize;
use serde_json::{json, Value};

pub const TRANSPORT_MEASUREMENT_REPORT_VERSION: u8 = 1;
pub const DEFAULT_MEASUREMENT_DURATION_SECONDS: u64 = 60;
pub const DEFAULT_STARTUP_TIMEOUT_SECONDS: u64 = 60;
pub const DURATION_SHORTFALL_TOLERANCE_US: u64 = 250_000;
pub const RATE_TOLERANCE_FRACTION: f64 = 0.05;
pub const MAX_SEQUENCE_LOSS_FRACTION: f64 = 0.001;
pub const DEFAULT_MEASUREMENT_DIRECTORY: &str = "artifacts/motion-measurements";
pub const HEADLESS_RUNTIME_REASON: &str =
    "headless_measurement_does_not_traverse_live_tilt_runtime";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransportMeasurementReportV1 {
    pub transport_measurement_report_version: u8,
    pub identification: Identification,
    pub measurement: Measurement,
    pub rates: Rates,
    pub timing: Timing,
    pub continuity: Continuity,
    pub arming_receiver_counters: ReceiverCounters,
    pub receiver_counters: ReceiverCounters,
    pub recorder_counters: RecorderCounters,
    pub runtime_metrics: NotApplicable,
    pub integrity_evaluation: IntegrityEvaluation,
    pub exploratory_baseline: ExploratoryBaseline,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identification {
    pub dataset_format_version: u8,
    pub protocol_version: u8,
    pub mounting_convention: String,
    pub methodology: String,
    pub temporal_metric_definitions: TemporalMetricDefinitions,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemporalMetricDefinitions {
    pub source_interval: String,
    pub receive_interarrival: String,
    pub relative_delay_variation: String,
    pub absolute_relative_delay_variation: String,
    pub population_stddev: String,
    pub percentile_method: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Measurement {
    pub requested_duration_us: u64,
    pub observed_duration_us: u64,
    pub expected_sample_rate_hz: u16,
    pub sample_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rates {
    pub source_average_rate_hz: AvailableRate,
    pub receive_average_rate_hz: AvailableRate,
    pub absolute_difference_hz: AvailableRate,
    pub relative_difference_fraction: AvailableRate,
    pub source_interval_count: usize,
    pub receive_interval_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum AvailableRate {
    Available {
        value: f64,
        unit: &'static str,
    },
    Unavailable {
        reason: &'static str,
        unit: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Timing {
    pub source_interval_us: Distribution,
    pub receive_interarrival_us: Distribution,
    pub relative_delay_variation_us: Distribution,
    pub absolute_relative_delay_variation_us: Distribution,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum Distribution {
    Available {
        unit: &'static str,
        count: usize,
        min: f64,
        mean: f64,
        population_stddev: f64,
        p50: f64,
        p95: f64,
        p99: f64,
        max: f64,
    },
    Unavailable {
        unit: &'static str,
        count: usize,
        reason: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Continuity {
    pub session_count: usize,
    pub session_ids: Vec<String>,
    pub sessions: Vec<ContinuitySession>,
    pub gap_event_count: usize,
    pub estimated_missing_samples: u64,
    pub sequence_loss: SequenceLoss,
    pub session_transitions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContinuitySession {
    pub session_id: String,
    pub first_sequence: u32,
    pub last_sequence: u32,
    pub observed_samples: usize,
    pub gap_events: usize,
    pub estimated_missing_samples: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum SequenceLoss {
    Available {
        #[serde(rename = "observedSamples")]
        observed_samples: u64,
        #[serde(rename = "estimatedMissingSamples")]
        estimated_missing_samples: u64,
        denominator: u64,
        fraction: f64,
    },
    Unavailable {
        reason: &'static str,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiverCounters {
    pub received_datagrams: u64,
    pub accepted_samples: u64,
    pub oversized_datagrams: u64,
    pub invalid_packets: u64,
    pub duplicate_or_out_of_order_packets: u64,
    pub foreign_session_packets: u64,
    pub rate_limited_datagrams: u64,
}

impl ReceiverCounters {
    pub fn delta(start: &ReceiverMetrics, end: &ReceiverMetrics) -> Self {
        Self {
            received_datagrams: end
                .received_datagrams
                .saturating_sub(start.received_datagrams),
            accepted_samples: end.accepted_samples.saturating_sub(start.accepted_samples),
            oversized_datagrams: end
                .oversized_datagrams
                .saturating_sub(start.oversized_datagrams),
            invalid_packets: end.invalid_packets.saturating_sub(start.invalid_packets),
            duplicate_or_out_of_order_packets: end
                .duplicate_or_out_of_order_packets
                .saturating_sub(start.duplicate_or_out_of_order_packets),
            foreign_session_packets: end
                .foreign_session_packets
                .saturating_sub(start.foreign_session_packets),
            rate_limited_datagrams: end
                .rate_limited_datagrams
                .saturating_sub(start.rate_limited_datagrams),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecorderCounters {
    pub received_accepted_samples: u64,
    pub written_samples: u64,
    pub recorder_dropped_samples: u64,
    pub completion: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotApplicable {
    NotApplicable { reason: &'static str },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrityEvaluation {
    pub passed: bool,
    pub gates: Vec<IntegrityGate>,
}

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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExploratoryBaseline {
    pub verdict: &'static str,
    pub timing: Timing,
    pub historical_context: Vec<String>,
}

pub fn build_report(
    analysis: &DatasetAnalysis,
    summary: &DatasetSummaryRecord,
    requested_duration_us: u64,
    expected_sample_rate_hz: u16,
    receiver_counters: ReceiverCounters,
) -> TransportMeasurementReportV1 {
    build_report_with_arming_counters(
        analysis,
        summary,
        requested_duration_us,
        expected_sample_rate_hz,
        ReceiverCounters::default(),
        receiver_counters,
    )
}

pub fn build_report_with_arming_counters(
    analysis: &DatasetAnalysis,
    summary: &DatasetSummaryRecord,
    requested_duration_us: u64,
    expected_sample_rate_hz: u16,
    arming_receiver_counters: ReceiverCounters,
    receiver_counters: ReceiverCounters,
) -> TransportMeasurementReportV1 {
    let timing = Timing {
        source_interval_us: distribution_u64(&analysis.source_interval_us),
        receive_interarrival_us: distribution_u64(&analysis.receive_interval_us),
        relative_delay_variation_us: distribution_i64(&analysis.relative_delay_variation_us),
        absolute_relative_delay_variation_us: distribution_u64(
            &analysis.absolute_relative_delay_variation_us,
        ),
    };
    let rates = build_rates(analysis);
    let observed_samples = analysis.sample_count as u64;
    let missing = analysis
        .sessions
        .iter()
        .map(|session| session.missing_sequence_total)
        .sum::<u64>();
    let denominator = observed_samples.saturating_add(missing);
    let sequence_loss = if denominator == 0 {
        SequenceLoss::Unavailable {
            reason: "no_observed_or_estimated_samples",
        }
    } else {
        SequenceLoss::Available {
            observed_samples,
            estimated_missing_samples: missing,
            denominator,
            fraction: missing as f64 / denominator as f64,
        }
    };
    let continuity = Continuity {
        session_count: analysis.session_count,
        session_ids: analysis
            .sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect(),
        sessions: analysis
            .sessions
            .iter()
            .map(|session| ContinuitySession {
                session_id: session.session_id.clone(),
                first_sequence: session.first_sequence,
                last_sequence: session.last_sequence,
                observed_samples: session.sample_count,
                gap_events: session.gap_count,
                estimated_missing_samples: session.missing_sequence_total,
            })
            .collect(),
        gap_event_count: analysis
            .sessions
            .iter()
            .map(|session| session.gap_count)
            .sum(),
        estimated_missing_samples: missing,
        sequence_loss,
        session_transitions: analysis.session_transition_count,
    };
    let recorder_counters = RecorderCounters {
        received_accepted_samples: summary.received_accepted_samples,
        written_samples: summary.written_samples,
        recorder_dropped_samples: summary.recorder_dropped_samples,
        completion: if summary.completed {
            "clean"
        } else {
            "interrupted"
        },
    };
    let gates = evaluate_gates(
        analysis,
        requested_duration_us,
        expected_sample_rate_hz,
        &rates,
        &continuity,
        &receiver_counters,
        &recorder_counters,
    );
    let passed = gates.iter().all(|gate| gate.result == GateResult::Passed);

    TransportMeasurementReportV1 {
        transport_measurement_report_version: TRANSPORT_MEASUREMENT_REPORT_VERSION,
        identification: Identification {
            dataset_format_version: analysis.dataset_format_version,
            protocol_version: analysis.protocol_version,
            mounting_convention: analysis.mounting_convention.clone(),
            methodology: "controlled headless window using the Rust UDP receiver and B1 NDJSON recorder, followed by deterministic offline analysis".to_owned(),
            temporal_metric_definitions: TemporalMetricDefinitions {
                source_interval: "difference between consecutive sessionElapsedUs values within one source session".to_owned(),
                receive_interarrival: "difference between consecutive receivedElapsedUs values for the same source session".to_owned(),
                relative_delay_variation: "receiveInterarrivalUs - sourceIntervalUs; signed values are preserved".to_owned(),
                absolute_relative_delay_variation: "abs(receiveInterarrivalUs - sourceIntervalUs); exploratory proxy of jitter only".to_owned(),
                population_stddev: "sqrt(sum((x - mean)^2) / count)".to_owned(),
                percentile_method: "linear R7 percentile".to_owned(),
            },
        },
        measurement: Measurement {
            requested_duration_us,
            observed_duration_us: analysis.observed_duration_us,
            expected_sample_rate_hz,
            sample_count: analysis.sample_count,
        },
        rates,
        timing: timing.clone(),
        continuity,
        arming_receiver_counters,
        receiver_counters,
        recorder_counters,
        runtime_metrics: NotApplicable::NotApplicable {
            reason: HEADLESS_RUNTIME_REASON,
        },
        integrity_evaluation: IntegrityEvaluation { passed, gates },
        exploratory_baseline: ExploratoryBaseline {
            verdict: "no_performance_verdict_first_physical_baseline",
            timing,
            historical_context: vec![
                "previous healthy captures observed receive interarrival p95 around 33-34 ms and p99 around 41 ms".to_owned(),
                "previous healthy captures observed signed relative delay variation p95 around 14-16 ms and p99 around 24 ms".to_owned(),
                "historical values are context, not thresholds".to_owned(),
            ],
        },
        limitations: vec![
            "source and receiver monotonic clocks have independent epochs and are never directly subtracted".to_owned(),
            "the report does not measure end-to-end latency, network latency, sensor-to-screen time, rendering time, or complete visual response".to_owned(),
            "percentiles, extrema, and population standard deviations are exploratory baseline only".to_owned(),
        ],
    }
}

fn build_rates(analysis: &DatasetAnalysis) -> Rates {
    let source = rate(
        analysis.source_average_rate_hz,
        analysis.source_interval_us.count,
    );
    let receive = rate(
        analysis.receive_average_rate_hz,
        analysis.receive_interval_us.count,
    );
    let (absolute, relative) = match (&source, &receive) {
        (
            AvailableRate::Available { value: source, .. },
            AvailableRate::Available { value: receive, .. },
        ) if *source > 0.0 => (
            AvailableRate::Available {
                value: (receive - source).abs(),
                unit: "Hz",
            },
            AvailableRate::Available {
                value: (receive - source).abs() / source,
                unit: "fraction",
            },
        ),
        _ => (
            AvailableRate::Unavailable {
                reason: "insufficient_intervals",
                unit: "Hz",
            },
            AvailableRate::Unavailable {
                reason: "insufficient_intervals",
                unit: "fraction",
            },
        ),
    };
    Rates {
        source_average_rate_hz: source,
        receive_average_rate_hz: receive,
        absolute_difference_hz: absolute,
        relative_difference_fraction: relative,
        source_interval_count: analysis.source_interval_us.count,
        receive_interval_count: analysis.receive_interval_us.count,
    }
}

fn rate(value: f64, count: usize) -> AvailableRate {
    if count > 0 && value.is_finite() && value > 0.0 {
        AvailableRate::Available { value, unit: "Hz" }
    } else {
        AvailableRate::Unavailable {
            reason: "insufficient_intervals",
            unit: "Hz",
        }
    }
}

fn distribution_u64(stats: &IntervalStats) -> Distribution {
    match (
        stats.min,
        stats.mean,
        stats.population_stddev,
        stats.p50,
        stats.p95,
        stats.p99,
        stats.max,
    ) {
        (Some(min), Some(mean), Some(stddev), Some(p50), Some(p95), Some(p99), Some(max)) => {
            Distribution::Available {
                unit: "microseconds",
                count: stats.count,
                min: min as f64,
                mean,
                population_stddev: stddev,
                p50,
                p95,
                p99,
                max: max as f64,
            }
        }
        _ => Distribution::Unavailable {
            unit: "microseconds",
            count: stats.count,
            reason: "insufficient_intervals",
        },
    }
}

fn distribution_i64(stats: &SignedIntervalStats) -> Distribution {
    match (
        stats.min,
        stats.mean,
        stats.population_stddev,
        stats.p50,
        stats.p95,
        stats.p99,
        stats.max,
    ) {
        (Some(min), Some(mean), Some(stddev), Some(p50), Some(p95), Some(p99), Some(max)) => {
            Distribution::Available {
                unit: "microseconds",
                count: stats.count,
                min: min as f64,
                mean,
                population_stddev: stddev,
                p50,
                p95,
                p99,
                max: max as f64,
            }
        }
        _ => Distribution::Unavailable {
            unit: "microseconds",
            count: stats.count,
            reason: "insufficient_intervals",
        },
    }
}

fn evaluate_gates(
    analysis: &DatasetAnalysis,
    requested_duration_us: u64,
    expected_rate: u16,
    rates: &Rates,
    continuity: &Continuity,
    receiver: &ReceiverCounters,
    recorder: &RecorderCounters,
) -> Vec<IntegrityGate> {
    let mut gates = Vec::new();
    let minimum_duration = requested_duration_us.saturating_sub(DURATION_SHORTFALL_TOLERANCE_US);
    gates.push(gate(
        "duration.minimum_observed",
        json!(analysis.observed_duration_us),
        format!("observedDurationUs >= {minimum_duration}"),
        analysis.observed_duration_us >= minimum_duration,
    ));
    gates.push(gate(
        "session.exactly_one",
        json!(continuity.session_count),
        "sessionCount == 1".to_owned(),
        continuity.session_count == 1,
    ));
    gates.push(gate(
        "session.no_transitions",
        json!(continuity.session_transitions),
        "sessionTransitions == 0".to_owned(),
        continuity.session_transitions == 0,
    ));
    let source_rate = available_value(&rates.source_average_rate_hz);
    let expected = f64::from(expected_rate);
    gates.push(gate(
        "rate.source_expected",
        source_rate.map_or(Value::Null, |value| json!(value)),
        format!("finite positive source rate within ±5% of {expected_rate} Hz"),
        source_rate
            .is_some_and(|value| ((value - expected) / expected).abs() <= RATE_TOLERANCE_FRACTION),
    ));
    let receive_rate = available_value(&rates.receive_average_rate_hz);
    let loss_fraction = sequence_loss_fraction(&continuity.sequence_loss);
    let expected_receive = source_rate
        .zip(loss_fraction)
        .map(|(value, loss)| value * (1.0 - loss));
    gates.push(gate(
        "rate.receive_loss_coherent",
        receive_rate.map_or(Value::Null, |value| json!(value)),
        "finite positive receive rate within ±5% of sourceRate * (1 - sequenceLossFraction)"
            .to_owned(),
        receive_rate
            .zip(expected_receive)
            .is_some_and(|(observed, expected)| {
                expected > 0.0
                    && ((observed - expected) / expected).abs() <= RATE_TOLERANCE_FRACTION
            }),
    ));
    gates.push(gate(
        "continuity.sequence_loss",
        serde_json::to_value(&continuity.sequence_loss)
            .unwrap_or_else(|_| json!({"availability": "unavailable", "reason": "serialization_error"})),
        format!("estimatedMissingSamples / (observedSamples + estimatedMissingSamples) <= {MAX_SEQUENCE_LOSS_FRACTION}"),
        loss_fraction.is_some_and(|fraction| fraction <= MAX_SEQUENCE_LOSS_FRACTION),
    ));
    for (id, value) in [
        ("receiver.oversized_zero", receiver.oversized_datagrams),
        ("receiver.invalid_zero", receiver.invalid_packets),
        (
            "receiver.duplicate_or_out_of_order_zero",
            receiver.duplicate_or_out_of_order_packets,
        ),
        (
            "receiver.foreign_session_zero",
            receiver.foreign_session_packets,
        ),
        (
            "receiver.rate_limited_zero",
            receiver.rate_limited_datagrams,
        ),
    ] {
        gates.push(gate(id, json!(value), "value == 0".to_owned(), value == 0));
    }
    gates.push(gate(
        "recorder.dropped_zero",
        json!(recorder.recorder_dropped_samples),
        "recorderDroppedSamples == 0".to_owned(),
        recorder.recorder_dropped_samples == 0,
    ));
    gates.push(gate(
        "path.accepted_samples_consistent",
        json!({
            "receiverAcceptedSamples": receiver.accepted_samples,
            "recorderReceivedAcceptedSamples": recorder.received_accepted_samples
        }),
        "receiverAcceptedSamples == recorderReceivedAcceptedSamples".to_owned(),
        receiver.accepted_samples == recorder.received_accepted_samples,
    ));
    gates.push(gate(
        "recorder.completed_cleanly",
        json!(recorder.completion),
        "completion == clean".to_owned(),
        recorder.completion == "clean",
    ));
    gates
}

fn available_value(rate: &AvailableRate) -> Option<f64> {
    match rate {
        AvailableRate::Available { value, .. } => Some(*value),
        AvailableRate::Unavailable { .. } => None,
    }
}

fn sequence_loss_fraction(loss: &SequenceLoss) -> Option<f64> {
    match loss {
        SequenceLoss::Available { fraction, .. } => Some(*fraction),
        SequenceLoss::Unavailable { .. } => None,
    }
}

fn gate(id: &'static str, observed: Value, expected: String, passed: bool) -> IntegrityGate {
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

pub fn format_human_report(report: &TransportMeasurementReportV1) -> String {
    let status = if report.integrity_evaluation.passed {
        "PASSED"
    } else {
        "FAILED"
    };
    let failed_gates = report
        .integrity_evaluation
        .gates
        .iter()
        .filter(|gate| gate.result == GateResult::Failed)
        .map(|gate| gate.id)
        .collect::<Vec<_>>();
    let failed_gates = if failed_gates.is_empty() {
        "none".to_owned()
    } else {
        failed_gates.join(", ")
    };
    let sequence_loss = sequence_loss_fraction(&report.continuity.sequence_loss)
        .map(|fraction| format!("{:.6}%", fraction * 100.0))
        .unwrap_or_else(|| "n/a".to_owned());
    let receive = format_percentiles(&report.timing.receive_interarrival_us);
    let relative = format_percentiles(&report.timing.relative_delay_variation_us);
    let absolute = format_percentiles(&report.timing.absolute_relative_delay_variation_us);
    format!(
        "Anchor transport measurement B5a v{}\nintegrity: {}\nfailedGates: {}\nsamples: {}\nobservedDurationUs: {}\nsessions: {}\nsequenceLoss: {}\nexploratory baseline (microseconds):\n  receiveInterarrivalUs: {}\n  relativeDelayVariationUs (signed): {}\n  absoluteRelativeDelayVariationUs (proxy of jitter): {}\nthese percentiles have no performance verdict\nruntime B4b: not_applicable ({})",
        report.transport_measurement_report_version,
        status,
        failed_gates,
        report.measurement.sample_count,
        report.measurement.observed_duration_us,
        report.continuity.session_count,
        sequence_loss,
        receive,
        relative,
        absolute,
        HEADLESS_RUNTIME_REASON,
    )
}

fn format_percentiles(distribution: &Distribution) -> String {
    match distribution {
        Distribution::Available { p50, p95, p99, .. } => {
            format!("p50={p50:.3} p95={p95:.3} p99={p99:.3}")
        }
        Distribution::Unavailable { .. } => "p50=n/a p95=n/a p99=n/a".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::analyze_dataset_str;

    fn dataset(
        sample_count: usize,
        duration_us: u64,
        sessions: usize,
        missing_after: Option<usize>,
        completed: bool,
    ) -> (DatasetAnalysis, DatasetSummaryRecord) {
        let mut text = String::from("{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-10-20T00:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n");
        let denominator = sample_count.saturating_sub(1).max(1) as u64;
        for index in 0..sample_count {
            let session_index = index * sessions / sample_count.max(1);
            let within_session = if sessions == 1 {
                index
            } else {
                index % (sample_count / sessions).max(1)
            };
            let mut sequence = within_session as u32;
            if missing_after.is_some_and(|missing| index > missing) {
                sequence += 1;
            }
            let elapsed = duration_us.saturating_mul(index as u64) / denominator;
            text.push_str(&format!("{{\"recordType\":\"sample\",\"receivedElapsedUs\":{elapsed},\"sample\":{{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-{session_index}\",\"sequence\":{sequence},\"sessionElapsedUs\":{elapsed},\"linearAccelerationMps2\":{{\"x\":0,\"y\":0,\"z\":0}},\"gravityMps2\":{{\"x\":0,\"y\":0,\"z\":-9.8}},\"angularVelocityRadS\":{{\"x\":0,\"y\":0,\"z\":0}}}}}}\n"));
        }
        text.push_str(&format!("{{\"recordType\":\"summary\",\"completed\":{completed},\"durationUs\":60000000,\"receivedAcceptedSamples\":{sample_count},\"writtenSamples\":{sample_count},\"recorderDroppedSamples\":0}}\n"));
        let analysis = analyze_dataset_str(&text).unwrap();
        let summary = DatasetSummaryRecord {
            record_type: crate::dataset::DatasetRecordType::Summary,
            completed,
            duration_us: 60_000_000,
            received_accepted_samples: sample_count as u64,
            written_samples: sample_count as u64,
            recorder_dropped_samples: 0,
        };
        (analysis, summary)
    }

    fn report(
        sample_count: usize,
        duration_us: u64,
        sessions: usize,
        missing_after: Option<usize>,
    ) -> TransportMeasurementReportV1 {
        let (analysis, summary) = dataset(sample_count, duration_us, sessions, missing_after, true);
        build_report(
            &analysis,
            &summary,
            60_000_000,
            60,
            ReceiverCounters {
                received_datagrams: sample_count as u64,
                accepted_samples: sample_count as u64,
                oversized_datagrams: 0,
                invalid_packets: 0,
                duplicate_or_out_of_order_packets: 0,
                foreign_session_packets: 0,
                rate_limited_datagrams: 0,
            },
        )
    }

    fn gate_result(report: &TransportMeasurementReportV1, id: &str) -> GateResult {
        report
            .integrity_evaluation
            .gates
            .iter()
            .find(|gate| gate.id == id)
            .unwrap()
            .result
    }

    fn report_for_session_order(session_ids: &[&str]) -> TransportMeasurementReportV1 {
        use std::collections::HashMap;
        let mut text = String::from("{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-10-20T00:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n");
        let mut sequences = HashMap::<&str, u32>::new();
        for (index, session_id) in session_ids.iter().enumerate() {
            let sequence = sequences.entry(session_id).or_default();
            let elapsed = index as u64 * 16_667;
            text.push_str(&format!("{{\"recordType\":\"sample\",\"receivedElapsedUs\":{elapsed},\"sample\":{{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"{session_id}\",\"sequence\":{sequence},\"sessionElapsedUs\":{elapsed},\"linearAccelerationMps2\":{{\"x\":0,\"y\":0,\"z\":0}},\"gravityMps2\":{{\"x\":0,\"y\":0,\"z\":-9.8}},\"angularVelocityRadS\":{{\"x\":0,\"y\":0,\"z\":0}}}}}}\n"));
            *sequence += 1;
        }
        text.push_str(&format!("{{\"recordType\":\"summary\",\"completed\":true,\"durationUs\":{},\"receivedAcceptedSamples\":{},\"writtenSamples\":{},\"recorderDroppedSamples\":0}}\n", session_ids.len().saturating_sub(1) as u64 * 16_667, session_ids.len(), session_ids.len()));
        let analysis = analyze_dataset_str(&text).unwrap();
        let summary = DatasetSummaryRecord {
            record_type: crate::dataset::DatasetRecordType::Summary,
            completed: true,
            duration_us: session_ids.len().saturating_sub(1) as u64 * 16_667,
            received_accepted_samples: session_ids.len() as u64,
            written_samples: session_ids.len() as u64,
            recorder_dropped_samples: 0,
        };
        build_report(
            &analysis,
            &summary,
            analysis.observed_duration_us,
            60,
            ReceiverCounters {
                received_datagrams: session_ids.len() as u64,
                accepted_samples: session_ids.len() as u64,
                oversized_datagrams: 0,
                invalid_packets: 0,
                duplicate_or_out_of_order_packets: 0,
                foreign_session_packets: 0,
                rate_limited_datagrams: 0,
            },
        )
    }

    #[test]
    fn known_statistics_include_population_stddev_r7_and_signed_negative_values() {
        let stats = SignedIntervalStats {
            count: 4,
            min: Some(-4),
            mean: Some(-1.0),
            population_stddev: Some(2.23606797749979),
            p50: Some(-1.0),
            p95: Some(1.7),
            p99: Some(1.94),
            max: Some(2),
        };
        let distribution = distribution_i64(&stats);
        assert!(
            matches!(distribution, Distribution::Available { count: 4, min, mean, population_stddev, p50, p95, p99, max, .. }
            if min == -4.0 && mean == -1.0 && (population_stddev - 2.23606797749979).abs() < 1e-12 && p50 == -1.0 && p95 == 1.7 && p99 == 1.94 && max == 2.0)
        );
    }

    #[test]
    fn absolute_relative_variation_is_reported() {
        let (analysis, _) = dataset(3, 20_000, 1, None, true);
        assert_eq!(analysis.absolute_relative_delay_variation_us.count, 2);
        assert_eq!(analysis.absolute_relative_delay_variation_us.max, Some(0));
    }

    #[test]
    fn healthy_single_session_passes_and_report_is_deterministic() {
        let a = report(3_600, 59_800_000, 1, None);
        let b = report(3_600, 59_800_000, 1, None);
        assert!(a.integrity_evaluation.passed);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        assert!(matches!(
            a.runtime_metrics,
            NotApplicable::NotApplicable {
                reason: HEADLESS_RUNTIME_REASON
            }
        ));

        let (analysis, summary) = dataset(3_600, 59_800_000, 1, None, true);
        let arming = ReceiverCounters {
            received_datagrams: 1,
            invalid_packets: 1,
            ..ReceiverCounters::default()
        };
        let window = ReceiverCounters {
            received_datagrams: 3_600,
            accepted_samples: 3_600,
            ..ReceiverCounters::default()
        };
        let with_arming_a = build_report_with_arming_counters(
            &analysis,
            &summary,
            60_000_000,
            60,
            arming.clone(),
            window.clone(),
        );
        let with_arming_b =
            build_report_with_arming_counters(&analysis, &summary, 60_000_000, 60, arming, window);
        assert_eq!(with_arming_a, with_arming_b);
        assert!(with_arming_a.integrity_evaluation.passed);
        assert_eq!(with_arming_a.arming_receiver_counters.invalid_packets, 1);
        assert_eq!(with_arming_a.receiver_counters.invalid_packets, 0);
    }

    #[test]
    fn two_sessions_and_materially_short_duration_fail_their_gates() {
        let report = report(3_600, 59_000_000, 2, None);
        assert_eq!(
            gate_result(&report, "session.exactly_one"),
            GateResult::Failed
        );
        assert_eq!(
            gate_result(&report, "session.no_transitions"),
            GateResult::Failed
        );
        assert_eq!(
            gate_result(&report, "duration.minimum_observed"),
            GateResult::Failed
        );
    }

    #[test]
    fn chronological_session_transitions_follow_sample_order() {
        for (order, expected) in [
            (vec!["A", "A", "A"], 0),
            (vec!["A", "B"], 1),
            (vec!["A", "B", "A"], 2),
        ] {
            let report = report_for_session_order(&order);
            assert_eq!(report.continuity.session_transitions, expected);
            assert_eq!(
                gate_result(&report, "session.no_transitions"),
                if expected == 0 {
                    GateResult::Passed
                } else {
                    GateResult::Failed
                }
            );
        }
    }

    #[test]
    fn duration_shortfall_at_tolerance_is_accepted() {
        let report = report(3_600, 59_750_000, 1, None);
        assert_eq!(
            gate_result(&report, "duration.minimum_observed"),
            GateResult::Passed
        );
    }

    #[test]
    fn gap_and_loss_denominator_are_explicit() {
        let at_threshold = report(999, 59_800_000, 1, Some(500));
        assert_eq!(at_threshold.continuity.gap_event_count, 1);
        assert_eq!(at_threshold.continuity.estimated_missing_samples, 1);
        assert!(matches!(
            at_threshold.continuity.sequence_loss,
            SequenceLoss::Available {
                denominator: 1_000,
                fraction,
                ..
            } if (fraction - 0.001).abs() < f64::EPSILON
        ));
        assert_eq!(
            gate_result(&at_threshold, "continuity.sequence_loss"),
            GateResult::Passed
        );
        let above = report(998, 59_800_000, 1, Some(500));
        assert!(matches!(
            above.continuity.sequence_loss,
            SequenceLoss::Available {
                denominator: 999,
                ..
            }
        ));
        assert_eq!(
            gate_result(&above, "continuity.sequence_loss"),
            GateResult::Failed
        );
    }

    #[test]
    fn source_rate_outside_tolerance_fails_while_receive_rate_is_loss_coherent() {
        let report = report(3_000, 59_800_000, 1, None);
        assert_eq!(
            gate_result(&report, "rate.source_expected"),
            GateResult::Failed
        );
        assert_eq!(
            gate_result(&report, "rate.receive_loss_coherent"),
            GateResult::Passed
        );
    }

    #[test]
    fn every_invalid_receiver_counter_and_recorder_drop_fail() {
        let (analysis, mut summary) = dataset(3_600, 59_800_000, 1, None, true);
        let fields = [
            "receiver.oversized_zero",
            "receiver.invalid_zero",
            "receiver.duplicate_or_out_of_order_zero",
            "receiver.foreign_session_zero",
            "receiver.rate_limited_zero",
        ];
        for (index, id) in fields.iter().enumerate() {
            let mut counters = ReceiverCounters {
                received_datagrams: 3_600,
                accepted_samples: 3_600,
                oversized_datagrams: 0,
                invalid_packets: 0,
                duplicate_or_out_of_order_packets: 0,
                foreign_session_packets: 0,
                rate_limited_datagrams: 0,
            };
            match index {
                0 => counters.oversized_datagrams = 1,
                1 => counters.invalid_packets = 1,
                2 => counters.duplicate_or_out_of_order_packets = 1,
                3 => counters.foreign_session_packets = 1,
                _ => counters.rate_limited_datagrams = 1,
            }
            let report = build_report(&analysis, &summary, 60_000_000, 60, counters);
            assert_eq!(gate_result(&report, id), GateResult::Failed);
        }
        summary.recorder_dropped_samples = 1;
        summary.received_accepted_samples += 1;
        let report = build_report(
            &analysis,
            &summary,
            60_000_000,
            60,
            ReceiverCounters {
                received_datagrams: 3_600,
                accepted_samples: 3_600,
                oversized_datagrams: 0,
                invalid_packets: 0,
                duplicate_or_out_of_order_packets: 0,
                foreign_session_packets: 0,
                rate_limited_datagrams: 0,
            },
        );
        assert_eq!(
            gate_result(&report, "recorder.dropped_zero"),
            GateResult::Failed
        );
    }

    #[test]
    fn distributions_without_intervals_are_unavailable_not_zero() {
        let (analysis, summary) = dataset(1, 0, 1, None, true);
        let report = build_report(
            &analysis,
            &summary,
            1,
            60,
            ReceiverCounters {
                received_datagrams: 1,
                accepted_samples: 1,
                oversized_datagrams: 0,
                invalid_packets: 0,
                duplicate_or_out_of_order_packets: 0,
                foreign_session_packets: 0,
                rate_limited_datagrams: 0,
            },
        );
        assert!(matches!(
            report.timing.source_interval_us,
            Distribution::Unavailable { count: 0, .. }
        ));
        assert!(matches!(
            report.rates.source_average_rate_hz,
            AvailableRate::Unavailable { .. }
        ));
    }

    #[test]
    fn sequence_loss_availability_never_uses_zero_for_missing_data() {
        let (mut analysis, mut summary) = dataset(2, 16_667, 1, None, true);
        analysis.sample_count = 0;
        analysis.sessions.clear();
        analysis.session_count = 0;
        analysis.session_transition_count = 0;
        summary.received_accepted_samples = 0;
        summary.written_samples = 0;
        let unavailable = build_report(
            &analysis,
            &summary,
            16_667,
            60,
            ReceiverCounters {
                received_datagrams: 0,
                accepted_samples: 0,
                oversized_datagrams: 0,
                invalid_packets: 0,
                duplicate_or_out_of_order_packets: 0,
                foreign_session_packets: 0,
                rate_limited_datagrams: 0,
            },
        );
        let loss_json = serde_json::to_value(&unavailable.continuity.sequence_loss).unwrap();
        assert_eq!(loss_json["availability"], "unavailable");
        assert_eq!(loss_json["reason"], "no_observed_or_estimated_samples");
        assert!(loss_json.get("fraction").is_none());
        assert_eq!(
            gate_result(&unavailable, "continuity.sequence_loss"),
            GateResult::Failed
        );
        assert_eq!(
            gate_result(&unavailable, "rate.receive_loss_coherent"),
            GateResult::Failed
        );
        let rate_gate = unavailable
            .integrity_evaluation
            .gates
            .iter()
            .find(|gate| gate.id == "continuity.sequence_loss")
            .unwrap();
        assert_eq!(rate_gate.observed["availability"], "unavailable");

        let available = report(3_600, 59_800_000, 1, None);
        let loss_json = serde_json::to_value(&available.continuity.sequence_loss).unwrap();
        assert_eq!(loss_json["availability"], "available");
        assert_eq!(loss_json["fraction"], 0.0);
        assert!(loss_json["denominator"].as_u64().unwrap() > 0);
    }

    #[test]
    fn human_output_lists_failed_gates_and_exploratory_percentiles() {
        let failed = report(3_000, 59_000_000, 1, None);
        let output = format_human_report(&failed);
        assert!(output.contains("failedGates: duration.minimum_observed, rate.source_expected"));
        assert!(output.contains("receiveInterarrivalUs: p50="));
        assert!(output.contains("relativeDelayVariationUs (signed): p50="));
        assert!(output.contains("absoluteRelativeDelayVariationUs (proxy of jitter): p50="));
        assert!(output.contains("these percentiles have no performance verdict"));

        let (mut analysis, summary) = dataset(1, 0, 1, None, true);
        analysis.sample_count = 0;
        analysis.sessions.clear();
        analysis.session_count = 0;
        let unavailable = build_report(
            &analysis,
            &summary,
            1,
            60,
            ReceiverCounters {
                received_datagrams: 1,
                accepted_samples: 1,
                oversized_datagrams: 0,
                invalid_packets: 0,
                duplicate_or_out_of_order_packets: 0,
                foreign_session_packets: 0,
                rate_limited_datagrams: 0,
            },
        );
        assert!(format_human_report(&unavailable).contains("p50=n/a p95=n/a p99=n/a"));
    }
}
