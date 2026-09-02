use anchor_desktop_lib::{
    dataset::{
        analyze_dataset_file, start_dataset_recorder, DatasetAnalysisError, DatasetRecorderConfig,
        DatasetRecorderFlowControl, RecordingScenario,
    },
    protocol::MAX_DATAGRAM_BYTES,
    receiver::{
        udp::{start_udp_receiver, UdpReceiverConfig},
        ReceiverState, SharedReceiverState,
    },
};
use std::{
    fs,
    net::UdpSocket as StdUdpSocket,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::UdpSocket, sync::watch, time::Instant};

const VALID_FIXTURE: &str =
    include_str!("../../../../packages/protocol/fixtures/valid/motion-sample.json");

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-fixtures/motion-datasets")
        .join(name)
}

fn temp_output_path(name: &str) -> PathBuf {
    let unique = format!(
        "anchor-dataset-test-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("unix epoch")
            .as_nanos()
    );
    std::env::temp_dir().join(unique).join("dataset.ndjson")
}

fn build_payload(sequence: u32, session_id: &str, session_elapsed_us: u64) -> Vec<u8> {
    let mut sample: serde_json::Value = serde_json::from_str(VALID_FIXTURE).expect("valid json");
    sample["sequence"] = serde_json::json!(sequence);
    sample["sessionId"] = serde_json::json!(session_id);
    sample["sessionElapsedUs"] = serde_json::json!(session_elapsed_us);
    serde_json::to_vec(&sample).expect("serialize payload")
}

fn write_dataset(name: &str, contents: &str) -> PathBuf {
    let path = temp_output_path(name);
    fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    fs::write(&path, contents).expect("write dataset");
    path
}

async fn start_recorder_and_receiver(
    output_name: &str,
    queue_capacity: usize,
    max_datagrams_per_second: u32,
) -> (
    anchor_desktop_lib::dataset::DatasetRecorderHandle,
    anchor_desktop_lib::receiver::udp::UdpReceiverHandle,
    SharedReceiverState,
) {
    let recorder = start_dataset_recorder(DatasetRecorderConfig {
        output_path: temp_output_path(output_name),
        scenario: RecordingScenario::Stationary,
        notes: None,
        queue_capacity,
        flow_control: None,
    })
    .await
    .expect("recorder should start");
    let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
    let (sample_tx, _sample_rx) = watch::channel(None);
    let receiver = start_udp_receiver(
        UdpReceiverConfig {
            bind_addr: "127.0.0.1:0".parse().expect("bind addr"),
            max_datagrams_per_second,
            accepted_sample_sink: Some(recorder.ingress()),
            ..UdpReceiverConfig::default()
        },
        shared_state.clone(),
        sample_tx,
    )
    .await
    .expect("receiver should start");

    (recorder, receiver, shared_state)
}

#[test]
fn analyze_dataset_reports_expected_statistics_for_complete_fixture() {
    let analysis = analyze_dataset_file(&fixture_path("complete-basic.ndjson"))
        .expect("complete fixture should analyze");

    assert_eq!(analysis.dataset_format_version, 1);
    assert!(analysis.is_complete);
    assert_eq!(analysis.sample_count, 3);
    assert_eq!(analysis.recorder_dropped_samples, 0);
    assert_eq!(analysis.sessions.len(), 1);
    assert_eq!(analysis.sessions[0].session_id, "session-a");
    assert_eq!(analysis.sessions[0].first_sequence, 0);
    assert_eq!(analysis.sessions[0].last_sequence, 3);
    assert_eq!(analysis.sessions[0].gap_count, 1);
    assert_eq!(analysis.sessions[0].missing_sequence_total, 1);
    assert_eq!(analysis.source_interval_us.min, Some(100_000));
    assert_eq!(analysis.source_interval_us.max, Some(200_000));
    assert_eq!(analysis.receive_interval_us.min, Some(120_000));
    assert_eq!(analysis.receive_interval_us.max, Some(210_000));
    assert_eq!(analysis.relative_delay_variation_us.min, Some(10_000));
    assert_eq!(analysis.relative_delay_variation_us.max, Some(20_000));
    assert!((analysis.gravity_magnitude.mean - 9.8).abs() < 1e-9);
    assert!(analysis.warnings.is_empty());
}

#[test]
fn analyze_dataset_warns_when_summary_is_missing() {
    let analysis = analyze_dataset_file(&fixture_path("interrupted-basic.ndjson"))
        .expect("interrupted fixture should still analyze");

    assert!(!analysis.is_complete);
    assert_eq!(analysis.sample_count, 2);
    assert_eq!(
        analysis.warnings,
        vec!["dataset summary record is missing; file may be interrupted".to_owned()]
    );
}

#[test]
fn analyze_dataset_rejects_invalid_json_lines() {
    let path = temp_output_path("invalid-line");
    fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    fs::write(
        &path,
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "not-json\n"
        ),
    )
    .expect("write invalid dataset");

    let err = analyze_dataset_file(&path).expect_err("invalid line must fail");
    assert!(matches!(
        err,
        DatasetAnalysisError::InvalidJsonLine { line: 2, .. }
    ));
}

#[test]
fn analyze_dataset_rejects_unsupported_dataset_version() {
    let path = temp_output_path("unsupported-version");
    fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    fs::write(
        &path,
        "{\"recordType\":\"metadata\",\"datasetFormatVersion\":2,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
    )
    .expect("write unsupported dataset");

    let err = analyze_dataset_file(&path).expect_err("unsupported version must fail");
    assert!(matches!(
        err,
        DatasetAnalysisError::UnsupportedDatasetVersion(2)
    ));
}

#[test]
fn analyze_dataset_rejects_received_elapsed_regression_without_panicking() {
    let path = write_dataset(
        "received-regression",
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":10,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":9,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":1,\"sessionElapsedUs\":1,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
        ),
    );

    let panic_result = std::panic::catch_unwind(|| analyze_dataset_file(&path));
    let result = panic_result.expect("analysis must not panic");
    let err = result.expect_err("receivedElapsedUs regression must fail");
    assert!(matches!(
        err,
        DatasetAnalysisError::ReceivedElapsedRegression {
            line: 3,
            previous_line: 2,
            previous_received_elapsed_us: 10,
            current_received_elapsed_us: 9,
        }
    ));
}

#[test]
fn analyze_dataset_rejects_session_elapsed_regression_without_panicking() {
    let path = write_dataset(
        "session-regression",
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":10,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":10,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":11,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":1,\"sessionElapsedUs\":9,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
        ),
    );

    let panic_result = std::panic::catch_unwind(|| analyze_dataset_file(&path));
    let result = panic_result.expect("analysis must not panic");
    let err = result.expect_err("sessionElapsedUs regression must fail");
    assert!(matches!(
        err,
        DatasetAnalysisError::SessionElapsedRegression {
            line: 3,
            previous_line: 2,
            ref session_id,
            previous_session_elapsed_us: 10,
            current_session_elapsed_us: 9,
        } if session_id == "session-a"
    ));
}

#[test]
fn analyze_dataset_accepts_sequence_at_u32_max() {
    let path = write_dataset(
        "u32-max-sequence",
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":10,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":4294967295,\"sessionElapsedUs\":10,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
        ),
    );

    let analysis = analyze_dataset_file(&path).expect("u32::MAX sequence should analyze");
    assert_eq!(analysis.sessions[0].first_sequence, u32::MAX);
    assert_eq!(analysis.sessions[0].last_sequence, u32::MAX);
}

#[test]
fn analyze_dataset_rejects_duplicate_metadata() {
    let path = write_dataset(
        "duplicate-metadata",
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n"
        ),
    );

    let err = analyze_dataset_file(&path).expect_err("duplicate metadata must fail");
    assert!(matches!(
        err,
        DatasetAnalysisError::InvalidDatasetStructure { line: 2, .. }
    ));
}

#[test]
fn analyze_dataset_rejects_sample_after_summary() {
    let path = write_dataset(
        "sample-after-summary",
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":0,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n",
            "{\"recordType\":\"summary\",\"completed\":true,\"durationUs\":0,\"receivedAcceptedSamples\":1,\"writtenSamples\":1,\"recorderDroppedSamples\":0}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":1,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":1,\"sessionElapsedUs\":1,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
        ),
    );

    let err = analyze_dataset_file(&path).expect_err("sample after summary must fail");
    assert!(matches!(
        err,
        DatasetAnalysisError::InvalidDatasetStructure { line: 4, .. }
    ));
}

#[test]
fn analyze_dataset_rejects_duplicate_summary() {
    let path = write_dataset(
        "duplicate-summary",
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":0,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n",
            "{\"recordType\":\"summary\",\"completed\":true,\"durationUs\":0,\"receivedAcceptedSamples\":1,\"writtenSamples\":1,\"recorderDroppedSamples\":0}\n",
            "{\"recordType\":\"summary\",\"completed\":true,\"durationUs\":0,\"receivedAcceptedSamples\":1,\"writtenSamples\":1,\"recorderDroppedSamples\":0}\n"
        ),
    );

    let err = analyze_dataset_file(&path).expect_err("duplicate summary must fail");
    assert!(matches!(
        err,
        DatasetAnalysisError::InvalidDatasetStructure { line: 4, .. }
    ));
}

#[test]
fn analyze_dataset_rejects_invalid_started_at_utc() {
    let path = write_dataset(
        "invalid-started-at",
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"not-rfc3339\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":0,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n"
        ),
    );

    let err = analyze_dataset_file(&path).expect_err("invalid startedAtUtc must fail");
    assert!(matches!(
        err,
        DatasetAnalysisError::InvalidMetadata { line: 1, .. }
    ));
}

#[test]
fn analyze_dataset_rejects_inconsistent_summary() {
    let path = write_dataset(
        "bad-summary",
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":0,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n",
            "{\"recordType\":\"summary\",\"completed\":true,\"durationUs\":0,\"receivedAcceptedSamples\":5,\"writtenSamples\":1,\"recorderDroppedSamples\":0}\n"
        ),
    );

    let err = analyze_dataset_file(&path).expect_err("inconsistent summary must fail");
    assert!(matches!(
        err,
        DatasetAnalysisError::InvalidSummary { line: 3, .. }
    ));
}

#[test]
fn analyze_dataset_warns_when_completed_false() {
    let path = write_dataset(
        "completed-false",
        concat!(
            "{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-02T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"flat_screen_up_portrait_top_toward_vehicle_front\"}\n",
            "{\"recordType\":\"sample\",\"receivedElapsedUs\":0,\"sample\":{\"protocolVersion\":1,\"kind\":\"motion_sample\",\"sessionId\":\"session-a\",\"sequence\":0,\"sessionElapsedUs\":0,\"linearAccelerationMps2\":{\"x\":0,\"y\":0,\"z\":0},\"gravityMps2\":{\"x\":0,\"y\":0,\"z\":-9.8},\"angularVelocityRadS\":{\"x\":0,\"y\":0,\"z\":0}}}\n",
            "{\"recordType\":\"summary\",\"completed\":false,\"durationUs\":0,\"receivedAcceptedSamples\":1,\"writtenSamples\":1,\"recorderDroppedSamples\":0}\n"
        ),
    );

    let analysis = analyze_dataset_file(&path).expect("completed=false dataset should analyze");
    assert_eq!(
        analysis.warnings,
        vec!["dataset summary indicates the recording ended early with completed=false".to_owned()]
    );
}

#[tokio::test]
async fn udp_receiver_records_only_accepted_samples_and_writes_summary() {
    let output_path = temp_output_path("accepted-only");
    let recorder = start_dataset_recorder(DatasetRecorderConfig {
        output_path: output_path.clone(),
        scenario: RecordingScenario::Stationary,
        notes: None,
        queue_capacity: 8,
        flow_control: None,
    })
    .await
    .expect("recorder should start");
    let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
    let (sample_tx, _sample_rx) = watch::channel(None);
    let receiver = start_udp_receiver(
        UdpReceiverConfig {
            bind_addr: "127.0.0.1:0".parse().expect("bind addr"),
            accepted_sample_sink: Some(recorder.ingress()),
            ..UdpReceiverConfig::default()
        },
        shared_state,
        sample_tx,
    )
    .await
    .expect("receiver should start");

    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("sender bind");
    socket
        .send_to(VALID_FIXTURE.as_bytes(), receiver.local_addr())
        .await
        .expect("send valid datagram");
    socket
        .send_to(VALID_FIXTURE.as_bytes(), receiver.local_addr())
        .await
        .expect("send duplicate datagram");
    socket
        .send_to(b"{", receiver.local_addr())
        .await
        .expect("send invalid datagram");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let written = recorder.snapshot().written_samples;
        if written == 1 {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "recorder did not write accepted sample in time"
        );
        tokio::task::yield_now().await;
    }

    receiver.shutdown().await;
    let summary = recorder.shutdown(true).await.expect("shutdown summary");

    assert_eq!(summary.received_accepted_samples, 1);
    assert_eq!(summary.written_samples, 1);
    assert_eq!(summary.recorder_dropped_samples, 0);

    let contents = fs::read_to_string(output_path).expect("dataset contents");
    assert_eq!(contents.lines().count(), 3);
    assert!(contents.contains("\"recordType\":\"summary\""));
}

#[tokio::test]
async fn udp_receiver_does_not_record_oversized_or_foreign_session_samples() {
    let (recorder, receiver, shared_state) =
        start_recorder_and_receiver("ignored-datagrams", 8, 240).await;
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("sender bind");

    let accepted_payload = build_payload(100, "session-a", 0);
    let foreign_payload = build_payload(0, "session-b", 10_000);
    let oversized_payload = vec![b'a'; MAX_DATAGRAM_BYTES + 1];

    socket
        .send_to(&accepted_payload, receiver.local_addr())
        .await
        .expect("send accepted datagram");
    socket
        .send_to(&foreign_payload, receiver.local_addr())
        .await
        .expect("send foreign datagram");
    socket
        .send_to(&oversized_payload, receiver.local_addr())
        .await
        .expect("send oversized datagram");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snapshot = shared_state
            .lock()
            .expect("shared state lock")
            .snapshot(std::time::Instant::now());
        if snapshot.metrics.accepted_samples == 1
            && snapshot.metrics.foreign_session_packets == 1
            && snapshot.metrics.oversized_datagrams == 1
        {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "receiver did not process ignored datagrams in time"
        );
        tokio::task::yield_now().await;
    }

    receiver.shutdown().await;
    let summary = recorder.shutdown(true).await.expect("shutdown summary");
    assert_eq!(summary.received_accepted_samples, 1);
    assert_eq!(summary.written_samples, 1);
}

#[tokio::test]
async fn udp_receiver_does_not_record_out_of_order_or_rate_limited_samples() {
    let (recorder, receiver, shared_state) =
        start_recorder_and_receiver("rate-limit", 512, 2).await;
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("sender bind");

    let accepted_payload = build_payload(100, "session-a", 0);
    let out_of_order_payload = build_payload(99, "session-a", 5_000);
    socket
        .send_to(&accepted_payload, receiver.local_addr())
        .await
        .expect("send accepted datagram");
    socket
        .send_to(&out_of_order_payload, receiver.local_addr())
        .await
        .expect("send out-of-order datagram");

    for sequence in 101..=103 {
        let payload = build_payload(sequence, "session-a", u64::from(sequence) * 16_000);
        socket
            .send_to(&payload, receiver.local_addr())
            .await
            .expect("send burst datagram");
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snapshot = shared_state
            .lock()
            .expect("shared state lock")
            .snapshot(std::time::Instant::now());
        if snapshot.metrics.duplicate_or_out_of_order_packets >= 1
            && snapshot.metrics.rate_limited_datagrams >= 1
        {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "receiver did not process rate-limited datagrams in time"
        );
        tokio::task::yield_now().await;
    }

    receiver.shutdown().await;
    let summary = recorder.shutdown(true).await.expect("shutdown summary");
    assert!(summary.received_accepted_samples < 4);
    assert_eq!(summary.written_samples, summary.received_accepted_samples);
}

#[tokio::test]
async fn recorder_queue_backpressure_drops_without_blocking_receiver() {
    let output_path = temp_output_path("queue-full");
    let flow_control = DatasetRecorderFlowControl::new_blocked();
    let recorder = start_dataset_recorder(DatasetRecorderConfig {
        output_path,
        scenario: RecordingScenario::Stationary,
        notes: None,
        queue_capacity: 1,
        flow_control: Some(flow_control.clone()),
    })
    .await
    .expect("recorder should start");
    let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
    let (sample_tx, _sample_rx) = watch::channel(None);
    let receiver = start_udp_receiver(
        UdpReceiverConfig {
            bind_addr: "127.0.0.1:0".parse().expect("bind addr"),
            accepted_sample_sink: Some(recorder.ingress()),
            ..UdpReceiverConfig::default()
        },
        shared_state.clone(),
        sample_tx,
    )
    .await
    .expect("receiver should start");

    let mut first_sample: serde_json::Value =
        serde_json::from_str(VALID_FIXTURE).expect("valid json");
    first_sample["sequence"] = serde_json::json!(100);
    first_sample["sessionElapsedUs"] = serde_json::json!(0);
    let first_payload = serde_json::to_vec(&first_sample).expect("serialize first payload");

    let mut second_sample = first_sample.clone();
    second_sample["sequence"] = serde_json::json!(101);
    second_sample["sessionElapsedUs"] = serde_json::json!(16_000);
    let second_payload = serde_json::to_vec(&second_sample).expect("serialize second payload");

    let mut third_sample = first_sample.clone();
    third_sample["sequence"] = serde_json::json!(102);
    third_sample["sessionElapsedUs"] = serde_json::json!(32_000);
    let third_payload = serde_json::to_vec(&third_sample).expect("serialize third payload");

    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("sender bind");
    socket
        .send_to(&first_payload, receiver.local_addr())
        .await
        .expect("send first datagram");
    flow_control.wait_until_writer_blocked().await;
    socket
        .send_to(&second_payload, receiver.local_addr())
        .await
        .expect("send second datagram");
    socket
        .send_to(&third_payload, receiver.local_addr())
        .await
        .expect("send third datagram");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let accepted = shared_state
            .lock()
            .expect("shared state lock")
            .snapshot(std::time::Instant::now())
            .metrics
            .accepted_samples;
        if accepted == 3 {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "receiver did not keep accepting samples in time"
        );
        tokio::task::yield_now().await;
    }

    assert_eq!(recorder.snapshot().written_samples, 0);
    assert_eq!(recorder.snapshot().recorder_dropped_samples, 1);

    flow_control.release();
    receiver.shutdown().await;
    let summary = recorder.shutdown(true).await.expect("shutdown summary");
    assert_eq!(summary.received_accepted_samples, 3);
    assert_eq!(summary.written_samples, 2);
    assert_eq!(summary.recorder_dropped_samples, 1);
}

#[tokio::test]
async fn start_dataset_recorder_reports_busy_port_with_desktop_guidance() {
    let occupied = StdUdpSocket::bind("127.0.0.1:0").expect("bind occupied socket");
    let bind_addr = occupied.local_addr().expect("occupied addr");
    let output_path = temp_output_path("busy-port");

    let recorder = start_dataset_recorder(DatasetRecorderConfig {
        output_path,
        scenario: RecordingScenario::Stationary,
        notes: None,
        queue_capacity: 8,
        flow_control: None,
    })
    .await
    .expect("recorder should start");

    let shared_state: SharedReceiverState = Arc::new(Mutex::new(ReceiverState::default()));
    let (sample_tx, _sample_rx) = watch::channel(None);
    let err = start_udp_receiver(
        UdpReceiverConfig {
            bind_addr,
            max_datagrams_per_second: 240,
            accepted_sample_sink: Some(recorder.ingress()),
            ..UdpReceiverConfig::default()
        },
        shared_state,
        sample_tx,
    )
    .await
    .expect_err("bind must fail");

    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
}
