use anchor_desktop_lib::{
    calibration::calibrate_dataset_str_at,
    dataset::{RecordingScenario, DEFAULT_MOUNTING_CONVENTION},
    motion_filtering::{
        evaluate_dataset_file, evaluate_synthetic_suite, format_human_evaluation, EvaluationConfig,
        EVALUATION_REPORT_VERSION,
    },
    protocol::{MotionSampleV1, PacketKind, ProtocolVersion, Vector3},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

#[test]
fn synthetic_report_is_deterministic_and_has_expected_configurations() {
    let config = EvaluationConfig::new(vec![50.0, 100.0], vec![100.0]).expect("config");
    let first = evaluate_synthetic_suite(&config).expect("first");
    let second = evaluate_synthetic_suite(&config).expect("second");
    assert_eq!(first.evaluation_report_version, EVALUATION_REPORT_VERSION);
    assert!(first.ground_truth_available);
    assert!(!first.yaw_calibrated);
    assert_eq!(first.configurations.len(), 4);
    assert_eq!(first.input.synthetic_fixtures.len(), 15);
    assert!(first
        .input
        .synthetic_fixtures
        .iter()
        .any(|fixture| fixture.name == "mounting_bias_b2"
            && fixture.calibration_kind == "mounting_bias_b2"));
    assert!(first
        .configurations
        .iter()
        .all(|configuration| configuration.fixture_results.len() == 15));
    let baseline = first
        .configurations
        .iter()
        .find(|configuration| configuration.candidate == "gravity_no_additional_anchor_filter")
        .expect("baseline");
    let roll_step = baseline
        .fixture_results
        .iter()
        .find(|item| item.fixture == "roll_step_positive")
        .expect("roll step");
    assert_metric_close(&roll_step.metrics.overshoot_deg, 0.0, 1e-9);
    let roll_sine = baseline
        .fixture_results
        .iter()
        .find(|item| item.fixture == "roll_sine")
        .expect("roll sine");
    assert_metric_close(&roll_sine.metrics.sine_lag_seconds, 0.0, 1e-9);
    assert_eq!(
        serde_json::to_string_pretty(&first).unwrap(),
        serde_json::to_string_pretty(&second).unwrap()
    );
    assert!(!serde_json::to_string(&first).unwrap().contains("NaN"));
    assert!(!serde_json::to_string(&first)
        .unwrap()
        .contains(std::env::current_dir().unwrap().to_str().unwrap()));
}

#[test]
fn physical_dataset_report_uses_proxy_metrics_only() {
    let dataset = write_dataset("physical", &stationary_dataset());
    let profile_path = write_profile_for_dataset(&dataset);
    let report = evaluate_dataset_file(
        &dataset,
        &profile_path,
        &EvaluationConfig::new(vec![50.0], vec![100.0]).unwrap(),
    )
    .expect("report");
    assert!(!report.ground_truth_available);
    assert_eq!(report.configurations.len(), 3);
    let json = serde_json::to_string_pretty(&report).unwrap();
    assert!(json.contains("ground truth unavailable for physical datasets"));
    assert!(!json.contains(dataset.parent().unwrap().to_str().unwrap()));
    assert!(report
        .configurations
        .iter()
        .all(|configuration| configuration.fixture_results.is_empty()));
    assert!(format_human_evaluation(&report).contains("behavioral proxies"));
    assert!(format_human_evaluation(&report).contains("winner: none"));
}

#[test]
fn cli_synthetic_human_and_json_work() {
    let human = run_cli(&[
        "evaluate",
        "--synthetic",
        "--low-pass-tau-ms",
        "50",
        "--complementary-tau-ms",
        "100",
    ]);
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let stdout = String::from_utf8(human.stdout).unwrap();
    assert!(stdout.contains("Anchor Motion Filter Evaluation v1"));
    assert!(stdout.contains("winner: none selected in B3a"));

    let json = run_cli(&[
        "evaluate",
        "--synthetic",
        "--low-pass-tau-ms",
        "50",
        "--complementary-tau-ms",
        "100",
        "--json",
    ]);
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let stdout = String::from_utf8(json.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("clean json");
    assert_eq!(value["evaluationReportVersion"], serde_json::json!(1));
}

#[test]
fn cli_dataset_json_and_error_paths_work() {
    let dataset = write_dataset("cli-physical", &stationary_dataset());
    let profile = write_profile_for_dataset(&dataset);
    let ok = run_cli(&[
        "evaluate",
        "--dataset",
        dataset.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--low-pass-tau-ms",
        "50",
        "--complementary-tau-ms",
        "100",
        "--json",
    ]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&ok.stdout).expect("json");
    assert_eq!(value["groundTruthAvailable"], serde_json::json!(false));

    let missing_profile = run_cli(&["evaluate", "--dataset", dataset.to_str().unwrap()]);
    assert!(!missing_profile.status.success());
    assert!(
        String::from_utf8_lossy(&missing_profile.stderr).contains("--dataset requires --profile")
    );

    let exclusive = run_cli(&[
        "evaluate",
        "--synthetic",
        "--dataset",
        dataset.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
    ]);
    assert!(!exclusive.status.success());
    assert!(String::from_utf8_lossy(&exclusive.stderr).contains("mutually exclusive"));

    for args in [
        vec![
            "evaluate",
            "--synthetic",
            "--low-pass-tau-ms",
            "",
            "--complementary-tau-ms",
            "100",
        ],
        vec![
            "evaluate",
            "--synthetic",
            "--low-pass-tau-ms",
            "10abc",
            "--complementary-tau-ms",
            "100",
        ],
        vec![
            "evaluate",
            "--synthetic",
            "--low-pass-tau-ms",
            "0",
            "--complementary-tau-ms",
            "100",
        ],
        vec!["evaluate", "--synthetic", "--unknown"],
        vec!["evaluate", "--synthetic", "--synthetic"],
        vec!["evaluate", "--profile", profile.to_str().unwrap()],
        vec![
            "evaluate",
            "--synthetic",
            "--low-pass-tau-ms",
            "50,50",
            "--complementary-tau-ms",
            "100",
        ],
        vec![
            "evaluate",
            "--synthetic",
            "--low-pass-tau-ms",
            "50,,100",
            "--complementary-tau-ms",
            "100",
        ],
        vec![
            "evaluate",
            "--synthetic",
            "--low-pass-tau-ms",
            "-50",
            "--complementary-tau-ms",
            "100",
        ],
        vec![
            "evaluate",
            "--synthetic",
            "--low-pass-tau-ms",
            "NaN",
            "--complementary-tau-ms",
            "100",
        ],
        vec![
            "evaluate",
            "--synthetic",
            "--low-pass-tau-ms",
            "Infinity",
            "--complementary-tau-ms",
            "100",
        ],
    ] {
        let out = run_cli(&args);
        assert!(!out.status.success(), "{args:?} should fail");
    }
}

fn assert_metric_close(
    metric: &anchor_desktop_lib::motion_filtering::report::MetricValue,
    expected: f64,
    tolerance: f64,
) {
    let actual = metric.value().expect("metric available");
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected} +/- {tolerance}, got {actual}"
    );
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_anchor-motion-dataset"))
        .args(args)
        .output()
        .expect("run cli")
}

fn write_profile_for_dataset(dataset: &Path) -> PathBuf {
    let report = calibrate_dataset_str_at(
        &fs::read_to_string(dataset).unwrap(),
        "stationary.ndjson",
        fixed_time(),
    )
    .expect("calibrate");
    let path = temp_dir("profile").join("profile.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        serde_json::to_string_pretty(&report.profile).unwrap(),
    )
    .unwrap();
    path
}

fn write_dataset(name: &str, contents: &str) -> PathBuf {
    let path = temp_dir(name).join("stationary.ndjson");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

fn stationary_dataset() -> String {
    let mut lines = vec![format!("{{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"{}\",\"startedAtUtc\":\"2026-09-03T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"{}\"}}", RecordingScenario::Stationary.as_str(), DEFAULT_MOUNTING_CONVENTION)];
    for i in 0..240_u32 {
        let sample = MotionSampleV1 {
            protocol_version: ProtocolVersion::V1,
            kind: PacketKind::MotionSample,
            session_id: "session-a".to_owned(),
            sequence: i,
            session_elapsed_us: u64::from(i) * 16_667,
            linear_acceleration_mps2: v(0.0, 0.0, 0.0),
            gravity_mps2: v(0.0, 0.0, -9.80665),
            angular_velocity_rad_s: v(0.0, 0.0, 0.0),
        };
        lines.push(format!(
            "{{\"recordType\":\"sample\",\"receivedElapsedUs\":{},\"sample\":{}}}",
            u64::from(i) * 16_667,
            serde_json::to_string(&sample).unwrap()
        ));
    }
    lines.push("{\"recordType\":\"summary\",\"completed\":true,\"durationUs\":3983413,\"receivedAcceptedSamples\":240,\"writtenSamples\":240,\"recorderDroppedSamples\":0}".to_owned());
    lines.join("\n") + "\n"
}
fn v(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3 { x, y, z }
}
fn fixed_time() -> SystemTime {
    time::OffsetDateTime::parse(
        "2026-09-03T12:00:01Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
    .into()
}
fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "anchor-b3a-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
