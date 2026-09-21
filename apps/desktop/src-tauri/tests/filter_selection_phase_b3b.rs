use anchor_desktop_lib::{
    calibration::calibrate_dataset_str_at,
    dataset::{RecordingScenario, DEFAULT_MOUNTING_CONVENTION},
    motion_filtering::{
        selection::{
            format_human_selection, select_tilt_estimator, PhysicalDatasetSelectionInput,
            SelectionConfig, SelectionStatus, TiltEstimatorPolicyV1,
            DEFAULT_SELECTION_COMPLEMENTARY_TAU_MS, DEFAULT_SELECTION_LOW_PASS_TAU_MS,
        },
        EvaluationConfig, EVALUATION_REPORT_VERSION,
    },
    protocol::{MotionSampleV1, PacketKind, ProtocolVersion, Vector3},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const REAL_PROFILE: &str =
    "artifacts/motion-calibrations/20260902t221420z-stationary-calibration-v1.json";
const REAL_DATASETS: &[(RecordingScenario, &str)] = &[
    (
        RecordingScenario::Stationary,
        "artifacts/motion-datasets/20260902T221420Z-stationary.ndjson",
    ),
    (
        RecordingScenario::RollRight,
        "artifacts/motion-datasets/20260902T221919Z-roll_right.ndjson",
    ),
    (
        RecordingScenario::RollLeft,
        "artifacts/motion-datasets/20260902T221937Z-roll_left.ndjson",
    ),
    (
        RecordingScenario::PitchFrontDown,
        "artifacts/motion-datasets/20260902T222109Z-pitch_front_down.ndjson",
    ),
    (
        RecordingScenario::PitchFrontUp,
        "artifacts/motion-datasets/20260902T222126Z-pitch_front_up.ndjson",
    ),
    (
        RecordingScenario::YawClockwise,
        "artifacts/motion-datasets/20260902T222326Z-yaw_clockwise.ndjson",
    ),
    (
        RecordingScenario::YawCounterclockwise,
        "artifacts/motion-datasets/20260902T222358Z-yaw_counterclockwise.ndjson",
    ),
    (
        RecordingScenario::LinearForward,
        "artifacts/motion-datasets/20260902T222523Z-linear_forward.ndjson",
    ),
    (
        RecordingScenario::LinearBackward,
        "artifacts/motion-datasets/20260902T222735Z-linear_backward.ndjson",
    ),
];

#[test]
fn selection_report_is_deterministic_and_contract_safe() {
    let fixture = write_selection_fixture("deterministic");
    let config = SelectionConfig::new(
        fixture.profile.clone(),
        fixture.datasets.clone(),
        vec![50.0, 100.0],
        vec![75.0, 150.0],
    )
    .expect("config");
    let first = select_tilt_estimator(config.clone()).expect("first selection");
    let second = select_tilt_estimator(config).expect("second selection");
    assert_eq!(first.selection_report_version, 1);
    assert_eq!(first.status, SelectionStatus::Inconclusive);
    assert_eq!(
        serde_json::to_string_pretty(&first).unwrap(),
        serde_json::to_string_pretty(&second).unwrap()
    );
    let json = serde_json::to_string_pretty(&first).unwrap();
    assert!(!json.contains("NaN"));
    assert!(!json.contains("Infinity"));
    assert!(!json.contains(fixture.root.to_str().unwrap()));
    assert!(json.contains("selectionReportVersion"));
    assert!(json.contains("groundTruthAvailable"));
    assert!(json.contains("paretoFront"));
    assert!(format_human_selection(&first).contains("physical metrics are proxies only"));
    assert!(first.recommendation.is_none());
    assert!(first.inconclusive.is_some());
}

#[test]
fn selection_rejects_missing_duplicate_and_invalid_grids() {
    let fixture = write_selection_fixture("invalid-inputs");
    let mut missing = fixture.datasets.clone();
    missing.pop();
    assert!(
        SelectionConfig::new(fixture.profile.clone(), missing, vec![50.0], vec![100.0]).is_err()
    );

    let mut duplicate = fixture.datasets.clone();
    duplicate[1].scenario = duplicate[0].scenario;
    assert!(
        SelectionConfig::new(fixture.profile.clone(), duplicate, vec![50.0], vec![100.0]).is_err()
    );

    assert!(SelectionConfig::new(
        fixture.profile,
        fixture.datasets,
        vec![50.0, 50.0],
        vec![100.0]
    )
    .is_err());
}

#[test]
fn cli_select_json_human_and_error_paths_work() {
    let fixture = write_selection_fixture("cli");
    let mut args = vec![
        "select".to_owned(),
        "--profile".to_owned(),
        fixture.profile.display().to_string(),
    ];
    for input in &fixture.datasets {
        args.push("--dataset".to_owned());
        args.push(format!(
            "{}={}",
            input.scenario.as_str(),
            input.path.display()
        ));
    }
    args.extend([
        "--low-pass-tau-ms".to_owned(),
        "50".to_owned(),
        "--complementary-tau-ms".to_owned(),
        "100".to_owned(),
    ]);
    let human = run_cli(&args.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    assert!(String::from_utf8(human.stdout)
        .unwrap()
        .contains("Anchor Motion Filter Selection v1"));

    let mut json_args = args.clone();
    json_args.push("--json".to_owned());
    let json = run_cli(&json_args.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).expect("clean json");
    assert_eq!(value["selectionReportVersion"], serde_json::json!(1));

    let duplicate = run_cli(&[
        "select",
        "--profile",
        fixture.profile.to_str().unwrap(),
        "--dataset",
        "stationary=a.ndjson",
        "--dataset",
        "stationary=b.ndjson",
    ]);
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("duplicate physical scenario"));

    let unknown = run_cli(&[
        "select",
        "--profile",
        fixture.profile.to_str().unwrap(),
        "--dataset",
        "unknown=a.ndjson",
    ]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("scenario must be one of"));
}

#[test]
fn policy_json_rejects_unknown_fields_version_and_invalid_parameters() {
    let valid = serde_json::json!({
        "version": 1,
        "candidate": "complementary_gravity_gyro",
        "parameters": { "correctionTauMs": 100.0 },
        "yawAvailable": false,
        "source": "b3b_offline_selection"
    });
    let policy: TiltEstimatorPolicyV1 = serde_json::from_value(valid).expect("policy json");
    policy.validate().expect("valid policy");

    let unknown = serde_json::json!({
        "version": 1,
        "candidate": "complementary_gravity_gyro",
        "parameters": { "correctionTauMs": 100.0 },
        "yawAvailable": false,
        "source": "b3b_offline_selection",
        "extra": true
    });
    assert!(serde_json::from_value::<TiltEstimatorPolicyV1>(unknown).is_err());

    let mut bad_version = policy.clone();
    bad_version.version = 2;
    assert!(bad_version.validate().is_err());
    let mut bad_parameter = policy;
    bad_parameter
        .parameters
        .insert("correctionTauMs".to_owned(), 0.0);
    assert!(bad_parameter.validate().is_err());
}

#[test]
fn b3a_contract_version_is_preserved() {
    assert_eq!(EVALUATION_REPORT_VERSION, 1);
    let report = anchor_desktop_lib::motion_filtering::evaluate_synthetic_suite(
        &EvaluationConfig::new(vec![50.0], vec![100.0]).unwrap(),
    )
    .expect("b3a synthetic report");
    assert_eq!(report.evaluation_report_version, 1);
}

#[test]
fn default_selection_grid_matches_b3b_required_grid() {
    assert_eq!(
        DEFAULT_SELECTION_LOW_PASS_TAU_MS,
        &[25.0, 50.0, 75.0, 100.0, 150.0, 200.0, 300.0, 400.0, 600.0, 800.0]
    );
    assert_eq!(
        DEFAULT_SELECTION_COMPLEMENTARY_TAU_MS,
        &[50.0, 75.0, 100.0, 150.0, 250.0, 400.0, 600.0, 1000.0, 1500.0, 2000.0]
    );
}

#[test]
#[ignore = "requires ignored local B1/B2 artifacts"]
fn real_b3b_selection_with_nine_physical_captures() {
    let profile = repo_root().join(REAL_PROFILE);
    if !profile.exists() {
        panic!("missing required B2 profile artifact: {REAL_PROFILE}");
    }
    let datasets = REAL_DATASETS
        .iter()
        .map(|(scenario, path)| {
            let full_path = repo_root().join(path);
            if !full_path.exists() {
                panic!("missing required B1 dataset artifact: {path}");
            }
            PhysicalDatasetSelectionInput {
                scenario: *scenario,
                path: full_path,
            }
        })
        .collect::<Vec<_>>();
    let report = select_tilt_estimator(
        SelectionConfig::new(
            profile,
            datasets,
            DEFAULT_SELECTION_LOW_PASS_TAU_MS.to_vec(),
            DEFAULT_SELECTION_COMPLEMENTARY_TAU_MS.to_vec(),
        )
        .expect("real config"),
    )
    .expect("real selection");
    assert_eq!(report.selection_report_version, 1);
}

struct SelectionFixture {
    root: PathBuf,
    profile: PathBuf,
    datasets: Vec<PhysicalDatasetSelectionInput>,
}

fn write_selection_fixture(name: &str) -> SelectionFixture {
    let root = temp_dir(name);
    fs::create_dir_all(&root).unwrap();
    let stationary = dataset_contents(RecordingScenario::Stationary);
    let profile_report = calibrate_dataset_str_at(&stationary, "stationary.ndjson", fixed_time())
        .expect("calibration profile");
    let profile = root.join("profile.json");
    fs::write(
        &profile,
        serde_json::to_string_pretty(&profile_report.profile).unwrap(),
    )
    .unwrap();
    let datasets = all_scenarios()
        .into_iter()
        .map(|scenario| {
            let path = root.join(format!("{}.ndjson", scenario.as_str()));
            fs::write(&path, dataset_contents(scenario)).unwrap();
            PhysicalDatasetSelectionInput { scenario, path }
        })
        .collect();
    SelectionFixture {
        root,
        profile,
        datasets,
    }
}

fn all_scenarios() -> Vec<RecordingScenario> {
    vec![
        RecordingScenario::Stationary,
        RecordingScenario::RollRight,
        RecordingScenario::RollLeft,
        RecordingScenario::PitchFrontDown,
        RecordingScenario::PitchFrontUp,
        RecordingScenario::YawClockwise,
        RecordingScenario::YawCounterclockwise,
        RecordingScenario::LinearForward,
        RecordingScenario::LinearBackward,
    ]
}

fn dataset_contents(scenario: RecordingScenario) -> String {
    let mut lines = vec![format!("{{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"{}\",\"startedAtUtc\":\"2026-09-03T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"{}\"}}", scenario.as_str(), DEFAULT_MOUNTING_CONVENTION)];
    for i in 0..240_u32 {
        let t = i as f64 / 60.0;
        let gravity = match scenario {
            RecordingScenario::RollRight => v(0.2 * t.sin(), 0.0, -9.80665),
            RecordingScenario::RollLeft => v(-0.2 * t.sin(), 0.0, -9.80665),
            RecordingScenario::PitchFrontDown => v(0.0, 0.2 * t.sin(), -9.80665),
            RecordingScenario::PitchFrontUp => v(0.0, -0.2 * t.sin(), -9.80665),
            _ => v(0.0, 0.0, -9.80665),
        };
        let angular = match scenario {
            RecordingScenario::YawClockwise => v(0.0, 0.0, -0.02 * t.sin()),
            RecordingScenario::YawCounterclockwise => v(0.0, 0.0, 0.02 * t.sin()),
            _ => v(0.0, 0.0, 0.0),
        };
        let linear = match scenario {
            RecordingScenario::LinearForward => v(0.0, 0.1 * t.sin(), 0.0),
            RecordingScenario::LinearBackward => v(0.0, -0.1 * t.sin(), 0.0),
            _ => v(0.0, 0.0, 0.0),
        };
        let sample = MotionSampleV1 {
            protocol_version: ProtocolVersion::V1,
            kind: PacketKind::MotionSample,
            session_id: "session-a".to_owned(),
            sequence: i,
            session_elapsed_us: u64::from(i) * 16_667,
            linear_acceleration_mps2: linear,
            gravity_mps2: gravity,
            angular_velocity_rad_s: angular,
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

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_anchor-motion-dataset"))
        .args(args)
        .output()
        .expect("run cli")
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
        "anchor-b3b-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}
