use anchor_desktop_lib::{
    calibration::{
        apply_calibration_to_angular, apply_calibration_to_gravity, apply_calibration_to_linear,
        calibrate_dataset_str, calibrate_dataset_str_at, default_profile_output_path_for_dataset,
        format_human_report, load_calibration_profile_file, load_calibration_profile_str,
        write_calibration_profile_file, CalibrationError, CALIBRATION_PROFILE_VERSION,
    },
    dataset::{RecordingScenario, DEFAULT_MOUNTING_CONVENTION},
    protocol::{MotionSampleV1, PacketKind, ProtocolVersion, Vector3},
};
use std::{
    f64::consts::PI,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Barrier},
    thread,
    time::SystemTime,
};

const EPS: f64 = 1e-9;
type ProfileMutation = Box<dyn Fn(&mut serde_json::Value)>;

#[test]
fn identity_with_perfectly_leveled_phone() {
    let dataset = build_stationary_dataset(StationarySpec::default());

    let report = calibrate_dataset_str(&dataset, "stationary.ndjson").expect("calibration");

    assert!(report.profile.quality.passed);
    assert!(!report.profile.yaw_calibrated);
    assert_close(report.profile.device_to_leveled_quaternion.w, 1.0, EPS);
    assert_close(report.profile.device_to_leveled_quaternion.x, 0.0, EPS);
    assert_close(report.profile.device_to_leveled_quaternion.y, 0.0, EPS);
    assert_close(report.profile.device_to_leveled_quaternion.z, 0.0, EPS);
    assert_close(report.profile.tilt_correction_degrees, 0.0, 1e-6);
}

#[test]
fn known_roll_tilt_is_corrected() {
    let dataset = build_stationary_dataset(StationarySpec {
        gravity_device: rotate_vector(euler_rotation_deg(18.0, 0.0, 0.0), vec3(0.0, 0.0, -9.81)),
        ..StationarySpec::default()
    });

    let report = calibrate_dataset_str(&dataset, "roll.ndjson").expect("calibration");

    assert!(report.profile.quality.passed);
    assert_close(report.profile.tilt_correction_degrees, 18.0, 1e-6);
    assert_close(
        report.residuals.gravity_angular_error_degrees.mean,
        0.0,
        1e-6,
    );
}

#[test]
fn known_pitch_tilt_is_corrected() {
    let dataset = build_stationary_dataset(StationarySpec {
        gravity_device: rotate_vector(euler_rotation_deg(0.0, -12.5, 0.0), vec3(0.0, 0.0, -9.81)),
        ..StationarySpec::default()
    });

    let report = calibrate_dataset_str(&dataset, "pitch.ndjson").expect("calibration");

    assert!(report.profile.quality.passed);
    assert_close(report.profile.tilt_correction_degrees, 12.5, 1e-6);
}

#[test]
fn combined_roll_and_pitch_is_corrected() {
    let rotation = euler_rotation_deg(10.0, -7.5, 0.0);
    let dataset = build_stationary_dataset(StationarySpec {
        gravity_device: rotate_vector(rotation, vec3(0.0, 0.0, -9.81)),
        ..StationarySpec::default()
    });

    let report = calibrate_dataset_str(&dataset, "combined.ndjson").expect("calibration");

    assert!(report.profile.quality.passed);
    assert_close(report.residuals.gravity_calibrated_mean_mps2.z, -9.81, 1e-6);
    assert_close(
        report.residuals.gravity_angular_error_degrees.rms,
        0.0,
        1e-6,
    );
}

#[test]
fn yaw_indeterminacy_is_preserved_explicitly() {
    let gravity = rotate_vector(euler_rotation_deg(11.0, 6.0, 47.0), vec3(0.0, 0.0, -9.81));
    let dataset = build_stationary_dataset(StationarySpec {
        gravity_device: gravity,
        ..StationarySpec::default()
    });

    let report = calibrate_dataset_str(&dataset, "yaw-free.ndjson").expect("calibration");

    assert!(!report.profile.yaw_calibrated);
    assert_close(report.profile.device_to_leveled_quaternion.z, 0.0, 1e-6);
}

#[test]
fn stationary_biases_are_removed_correctly() {
    let dataset = build_stationary_dataset(StationarySpec {
        linear_bias: vec3(0.12, -0.08, 0.03),
        angular_bias: vec3(0.02, -0.01, 0.04),
        ..StationarySpec::default()
    });

    let report = calibrate_dataset_str(&dataset, "bias.ndjson").expect("calibration");

    assert!(report.profile.quality.passed);
    assert_close(report.residuals.linear_calibrated_mean_mps2.x, 0.0, 1e-9);
    assert_close(report.residuals.linear_calibrated_rms_mps2, 0.0, 1e-9);
    assert_close(report.residuals.angular_calibrated_mean_rad_s.z, 0.0, 1e-9);
    assert_close(report.residuals.angular_calibrated_rms_rad_s, 0.0, 1e-9);
}

#[test]
fn calibrated_gravity_aligns_to_negative_z() {
    let dataset = build_stationary_dataset(StationarySpec {
        gravity_device: rotate_vector(euler_rotation_deg(9.0, -13.0, 0.0), vec3(0.0, 0.0, -9.73)),
        ..StationarySpec::default()
    });

    let report = calibrate_dataset_str(&dataset, "gravity.ndjson").expect("calibration");

    assert_close(report.residuals.gravity_calibrated_mean_mps2.x, 0.0, 1e-6);
    assert_close(report.residuals.gravity_calibrated_mean_mps2.y, 0.0, 1e-6);
    assert_close(report.residuals.gravity_calibrated_mean_mps2.z, -9.73, 1e-6);
}

#[test]
fn quaternion_is_unitary_and_serialized_canonically() {
    let dataset = build_stationary_dataset(StationarySpec {
        gravity_device: rotate_vector(euler_rotation_deg(20.0, 0.0, 0.0), vec3(0.0, 0.0, -9.81)),
        ..StationarySpec::default()
    });

    let report = calibrate_dataset_str(&dataset, "canonical.ndjson").expect("calibration");
    let serialized = serde_json::to_string(&report.profile).expect("serialize profile");

    assert!(report.profile.device_to_leveled_quaternion.w >= 0.0);
    assert!(serialized.contains("\"deviceToLeveledQuaternion\":{"));
    assert!(serialized.contains("\"w\":"));
}

#[test]
fn calibration_is_deterministic_for_same_input() {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let created_at = fixed_profile_creation_time();

    let first = calibrate_dataset_str_at(&dataset, "same.ndjson", created_at).expect("first");
    let second = calibrate_dataset_str_at(&dataset, "same.ndjson", created_at).expect("second");

    assert_eq!(first.profile, second.profile);
    assert_eq!(first.residuals, second.residuals);
}

#[test]
fn profile_creation_time_is_distinct_from_source_started_at() {
    let dataset = build_stationary_dataset(StationarySpec::default());

    let report = calibrate_dataset_str_at(&dataset, "time.ndjson", fixed_profile_creation_time())
        .expect("calibration");

    assert_eq!(report.profile.created_at_utc, "2026-09-03T12:00:01Z");
    assert_eq!(report.profile.source_started_at_utc, "2026-09-03T12:00:00Z");
}

#[test]
fn non_stationary_scenario_is_rejected() {
    let dataset = build_dataset(BuildDatasetSpec {
        scenario: RecordingScenario::RollRight,
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "roll_right.ndjson").expect_err("must fail");

    assert_violation(&err, "scenario must be stationary");
}

#[test]
fn short_dataset_is_rejected() {
    let dataset = build_stationary_dataset(StationarySpec {
        sample_count: 179,
        ..StationarySpec::default()
    });

    let err = calibrate_dataset_str(&dataset, "short.ndjson").expect_err("must fail");

    assert_violation(&err, "sample count");
}

#[test]
fn incomplete_dataset_is_rejected() {
    let dataset = build_dataset(BuildDatasetSpec {
        completed: false,
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "incomplete.ndjson").expect_err("must fail");

    assert_violation(&err, "dataset must be complete");
}

#[test]
fn multiple_sessions_are_rejected() {
    let dataset = build_dataset(BuildDatasetSpec {
        split_session_at: Some(100),
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "multi-session.ndjson").expect_err("must fail");

    assert_violation(&err, "exactly one session");
}

#[test]
fn multiple_quality_gate_violations_are_reported_together() {
    let dataset = build_dataset(BuildDatasetSpec {
        scenario: RecordingScenario::YawClockwise,
        sample_count: 120,
        completed: false,
        recorder_dropped_samples: 2,
        session_interval_us: 10_000,
        received_interval_us: 10_000,
        gravity_device: vec3(0.0, 0.0, -8.5),
        angular_bias: vec3(0.0, 0.0, 0.2),
        split_session_at: Some(60),
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "many.ndjson").expect_err("must fail");

    match err {
        CalibrationError::QualityGateFailed(report) => {
            assert!(report.violations.len() >= 6);
        }
        other => panic!("expected quality gate error, got {other}"),
    }
}

#[test]
fn recorder_drops_are_rejected() {
    let dataset = build_dataset(BuildDatasetSpec {
        recorder_dropped_samples: 1,
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "drops.ndjson").expect_err("must fail");

    assert_violation(&err, "recorderDroppedSamples");
}

#[test]
fn sequence_gaps_above_one_percent_are_rejected() {
    let dataset = build_dataset(BuildDatasetSpec {
        sequence_jump_at: Some((100, 3)),
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "gaps.ndjson").expect_err("must fail");

    assert_violation(&err, "missing sequence");
}

#[test]
fn out_of_range_rate_is_rejected() {
    let dataset = build_dataset(BuildDatasetSpec {
        session_interval_us: 10_000,
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "rate.ndjson").expect_err("must fail");

    assert_violation(&err, "source average rate");
}

#[test]
fn implausible_gravity_magnitude_is_rejected() {
    let dataset = build_stationary_dataset(StationarySpec {
        gravity_device: vec3(0.0, 0.0, -8.5),
        ..StationarySpec::default()
    });

    let err = calibrate_dataset_str(&dataset, "gravity-low.ndjson").expect_err("must fail");

    assert_violation(&err, "gravity magnitude mean");
}

#[test]
fn unstable_gravity_is_rejected() {
    let dataset = build_dataset(BuildDatasetSpec {
        gravity_noise_amplitude: 0.11,
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "gravity-unstable.ndjson").expect_err("must fail");

    assert_violation(&err, "gravity magnitude stddev");
}

#[test]
fn excessive_linear_motion_is_rejected() {
    let dataset = build_dataset(BuildDatasetSpec {
        linear_bias: vec3(0.36, 0.0, 0.0),
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "linear-motion.ndjson").expect_err("must fail");

    assert_violation(&err, "linear acceleration RMS");
}

#[test]
fn excessive_rotation_is_rejected() {
    let dataset = build_dataset(BuildDatasetSpec {
        angular_bias: vec3(0.0, 0.0, 0.11),
        ..BuildDatasetSpec::stationary_defaults()
    });

    let err = calibrate_dataset_str(&dataset, "rotation.ndjson").expect_err("must fail");

    assert_violation(&err, "angular velocity RMS");
}

#[test]
fn tilt_above_thirty_degrees_is_rejected() {
    let dataset = build_stationary_dataset(StationarySpec {
        gravity_device: rotate_vector(euler_rotation_deg(31.0, 0.0, 0.0), vec3(0.0, 0.0, -9.81)),
        ..StationarySpec::default()
    });

    let err = calibrate_dataset_str(&dataset, "tilt-high.ndjson").expect_err("must fail");

    assert_violation(&err, "tilt correction");
}

#[test]
fn thresholds_accept_exact_boundary_values() {
    let dataset = build_dataset(BuildDatasetSpec {
        sample_count: 180,
        session_interval_us: 1_000_000 / 45,
        received_interval_us: 1_000_000 / 45,
        gravity_device: vec3(0.0, 0.0, -9.0),
        linear_bias: vec3(0.35, 0.0, 0.0),
        angular_bias: vec3(0.10, 0.0, 0.0),
        ..BuildDatasetSpec::stationary_defaults()
    });

    let report = calibrate_dataset_str(&dataset, "boundary.ndjson").expect("must pass");

    assert!(report.profile.quality.passed);
}

#[test]
fn unknown_profile_version_or_method_is_rejected() {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let report = calibrate_dataset_str(&dataset, "profile.ndjson").expect("calibration");
    let mut value = serde_json::to_value(&report.profile).expect("profile value");
    value["calibrationProfileVersion"] = serde_json::json!(2);

    let version_err = load_calibration_profile_str(&serde_json::to_string(&value).expect("json"))
        .expect_err("unknown version must fail");
    assert!(version_err
        .to_string()
        .contains("unsupported calibrationProfileVersion"));

    value["calibrationProfileVersion"] = serde_json::json!(CALIBRATION_PROFILE_VERSION);
    value["method"] = serde_json::json!("other_method");
    let method_err = load_calibration_profile_str(&serde_json::to_string(&value).expect("json"))
        .expect_err("unknown method must fail");
    assert!(method_err
        .to_string()
        .contains("unsupported calibration method"));
}

#[test]
fn unknown_profile_fields_are_rejected() {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let report = calibrate_dataset_str(&dataset, "profile.ndjson").expect("calibration");
    let mut value = serde_json::to_value(&report.profile).expect("profile value");
    value["unexpected"] = serde_json::json!(true);

    let err = load_calibration_profile_str(&serde_json::to_string(&value).expect("json"))
        .expect_err("unknown field must fail");

    assert!(err.to_string().contains("unknown field"));
}

#[test]
fn invalid_profile_quaternion_is_rejected() {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let report = calibrate_dataset_str(&dataset, "profile.ndjson").expect("calibration");
    let mut value = serde_json::to_value(&report.profile).expect("profile value");
    value["deviceToLeveledQuaternion"]["w"] = serde_json::json!(2.0);

    let err = load_calibration_profile_str(&serde_json::to_string(&value).expect("json"))
        .expect_err("invalid quaternion must fail");

    assert!(err.to_string().contains("unit quaternion"));
}

#[test]
fn tampered_profile_invariants_are_rejected() {
    let base = valid_profile_value();
    let cases: Vec<(&str, ProfileMutation)> = vec![
        (
            "created before source start",
            Box::new(|value| value["createdAtUtc"] = serde_json::json!("2026-09-03T11:59:59Z")),
        ),
        (
            "sourceDatasetFormatVersion",
            Box::new(|value| value["sourceDatasetFormatVersion"] = serde_json::json!(2)),
        ),
        (
            "sourceProtocolVersion",
            Box::new(|value| value["sourceProtocolVersion"] = serde_json::json!(2)),
        ),
        (
            "sourceSessionId",
            Box::new(|value| value["sourceSessionId"] = serde_json::json!("")),
        ),
        (
            "sourceSampleCount",
            Box::new(|value| value["sourceSampleCount"] = serde_json::json!(179)),
        ),
        (
            "sourceObservedDurationUs",
            Box::new(|value| value["sourceObservedDurationUs"] = serde_json::json!(2_999_999)),
        ),
        (
            "criteria",
            Box::new(|value| value["quality"]["criteria"] = serde_json::json!("other")),
        ),
        (
            "quality.passed",
            Box::new(|value| value["quality"]["passed"] = serde_json::json!(false)),
        ),
        (
            "quality.violations",
            Box::new(|value| {
                value["quality"]["violations"] =
                    serde_json::json!([{ "code": "x", "message": "bad" }])
            }),
        ),
        (
            "diagnostic limits",
            Box::new(|value| {
                value["quality"]["diagnostics"]["linearAccelerationRmsMps2"] =
                    serde_json::json!(0.351)
            }),
        ),
        (
            "sample count mismatch",
            Box::new(|value| {
                value["quality"]["diagnostics"]["sampleCount"] = serde_json::json!(181)
            }),
        ),
        (
            "tilt mismatch",
            Box::new(|value| {
                value["quality"]["diagnostics"]["tiltCorrectionDegrees"] = serde_json::json!(1.0)
            }),
        ),
        (
            "gravity mean mismatch",
            Box::new(|value| {
                value["quality"]["diagnostics"]["gravityMagnitudeMeanMps2"] = serde_json::json!(9.5)
            }),
        ),
        (
            "negative rms",
            Box::new(|value| {
                value["quality"]["residuals"]["linearCalibratedRmsMps2"] = serde_json::json!(-0.1)
            }),
        ),
        (
            "bad fraction",
            Box::new(|value| {
                value["quality"]["diagnostics"]["missingSequenceFraction"] = serde_json::json!(1.1)
            }),
        ),
        (
            "gravity quaternion mismatch",
            Box::new(|value| value["deviceToLeveledQuaternion"]["x"] = serde_json::json!(0.1)),
        ),
        (
            "mean gravity tilt mismatch",
            Box::new(|value| value["deviceFrameMeanGravityMps2"]["x"] = serde_json::json!(1.0)),
        ),
        (
            "linear bias contradiction",
            Box::new(|value| {
                value["deviceFrameLinearAccelerationBiasMps2"]["x"] = serde_json::json!(100.0)
            }),
        ),
        (
            "angular bias contradiction",
            Box::new(|value| {
                value["deviceFrameAngularVelocityBiasRadS"]["z"] = serde_json::json!(100.0)
            }),
        ),
        (
            "linear residual mean contradiction",
            Box::new(|value| {
                value["quality"]["residuals"]["linearCalibratedMeanMps2"]["x"] =
                    serde_json::json!(0.1)
            }),
        ),
        (
            "angular residual mean contradiction",
            Box::new(|value| {
                value["quality"]["residuals"]["angularCalibratedMeanRadS"]["y"] =
                    serde_json::json!(0.1)
            }),
        ),
        (
            "linear residual rms contradiction",
            Box::new(|value| {
                value["quality"]["residuals"]["linearCalibratedRmsMps2"] = serde_json::json!(0.2)
            }),
        ),
        (
            "gravity calibrated mean contradiction",
            Box::new(|value| {
                value["quality"]["residuals"]["gravityCalibratedMeanMps2"]["x"] =
                    serde_json::json!(0.1)
            }),
        ),
        (
            "gravity residual magnitude contradiction",
            Box::new(|value| {
                value["quality"]["residuals"]["gravityCalibratedMagnitudeMeanMps2"] =
                    serde_json::json!(9.5)
            }),
        ),
        (
            "mean gravity norm exceeds mean magnitude",
            Box::new(|value| value["gravityMagnitudeMeanMps2"] = serde_json::json!(9.0)),
        ),
        (
            "angular residual stats physical range",
            Box::new(|value| {
                value["quality"]["residuals"]["gravityAngularErrorDegrees"]["max"] =
                    serde_json::json!(181.0)
            }),
        ),
    ];

    for (name, mutate) in cases {
        let mut value = base.clone();
        mutate(&mut value);
        assert_tampered_profile_rejected_everywhere(name, value);
    }
}

#[test]
fn apply_api_rejects_non_finite_vectors() {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let profile =
        calibrate_dataset_str_at(&dataset, "profile.ndjson", fixed_profile_creation_time())
            .expect("calibration")
            .profile;

    assert!(apply_calibration_to_linear(&profile, &vec3(f64::NAN, 0.0, 0.0)).is_err());
    assert!(apply_calibration_to_angular(&profile, &vec3(0.0, f64::INFINITY, 0.0)).is_err());
    assert!(apply_calibration_to_gravity(&profile, &vec3(0.0, 0.0, f64::NEG_INFINITY)).is_err());
}

#[test]
fn extreme_values_do_not_overflow_or_panic() {
    let dataset = build_dataset(BuildDatasetSpec {
        sample_count: 181,
        session_interval_us: 50_000_000_000,
        received_interval_us: 50_000_000_000,
        gravity_device: vec3(0.0, 0.0, -10.5),
        ..BuildDatasetSpec::stationary_defaults()
    });

    let panic_result =
        std::panic::catch_unwind(|| calibrate_dataset_str(&dataset, "extreme.ndjson"));
    let result = panic_result.expect("must not panic");
    assert!(result.is_err());
}

#[test]
fn write_failure_does_not_leave_partial_file() {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let report = calibrate_dataset_str(&dataset, "profile.ndjson").expect("calibration");
    let base = temp_path("blocked-parent");
    fs::create_dir_all(&base).expect("dir");
    let blocking_file = base.join("not-a-directory");
    fs::write(&blocking_file, "block").expect("write blocking file");
    let output = blocking_file.join("profile.json");

    let result = write_calibration_profile_file(&output, &report.profile);

    assert!(result.is_err());
    assert!(!output.exists());
    assert_eq!(list_matching(&base, ".tmp").len(), 0);
}

#[test]
fn existing_output_is_not_overwritten() {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let report = calibrate_dataset_str(&dataset, "profile.ndjson").expect("calibration");
    let output = temp_path("existing-output").join("profile.json");
    fs::create_dir_all(output.parent().expect("parent")).expect("dir");
    fs::write(&output, "keep").expect("write existing");

    let err = write_calibration_profile_file(&output, &report.profile).expect_err("must refuse");

    assert!(err.to_string().contains("already exists"));
    assert_eq!(fs::read_to_string(&output).expect("read"), "keep");
}

#[test]
fn concurrent_profile_writes_publish_exactly_once_without_temp_residue() {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let profile = Arc::new(
        calibrate_dataset_str_at(&dataset, "profile.ndjson", fixed_profile_creation_time())
            .expect("calibration")
            .profile,
    );
    let base = temp_path("concurrent-output");
    fs::create_dir_all(&base).expect("dir");
    let output = Arc::new(base.join("profile.json"));
    let barrier = Arc::new(Barrier::new(2));

    let handles: Vec<_> = (0..2)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let output = Arc::clone(&output);
            let profile = Arc::clone(&profile);
            thread::spawn(move || {
                barrier.wait();
                write_calibration_profile_file(&output, &profile)
            })
        })
        .collect();

    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();
    let successes = results.iter().filter(|result| result.is_ok()).count();
    let already_exists = results
        .iter()
        .filter(|result| {
            matches!(result, Err(CalibrationError::Io(err)) if err.kind() == std::io::ErrorKind::AlreadyExists)
        })
        .count();

    assert_eq!(successes, 1);
    assert_eq!(already_exists, 1);
    let loaded = load_calibration_profile_file(&output).expect("valid final profile");
    assert_eq!(loaded.source_dataset, "profile.ndjson");
    assert_eq!(list_matching(&base, ".tmp").len(), 0);
}

#[test]
fn human_report_and_json_cli_are_supported() {
    let dataset_path = write_dataset_file(
        "cli-stationary",
        &build_stationary_dataset(StationarySpec::default()),
    );
    let output_path = temp_path("cli-output").join("profile.json");
    fs::create_dir_all(output_path.parent().expect("parent")).expect("dir");

    let human = run_cli(&[
        "calibrate",
        dataset_path.to_str().expect("dataset path"),
        "--output",
        output_path.to_str().expect("output path"),
    ]);
    assert!(human.status.success());
    let human_stdout = String::from_utf8(human.stdout).expect("utf8");
    assert!(human_stdout.contains("yawCalibrated: false"));
    assert!(human_stdout.contains("tiltCorrectionDegrees"));

    let json_output_path = temp_path("cli-output-json").join("profile.json");
    fs::create_dir_all(json_output_path.parent().expect("parent")).expect("dir");
    let json = run_cli(&[
        "calibrate",
        dataset_path.to_str().expect("dataset path"),
        "--output",
        json_output_path.to_str().expect("output path"),
        "--json",
    ]);
    assert!(json.status.success());
    let json_stdout = String::from_utf8(json.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&json_stdout).expect("valid json");
    assert_eq!(value["profile"]["yawCalibrated"], serde_json::json!(false));
}

#[test]
fn default_output_path_is_safe_and_relative() {
    let path = default_profile_output_path_for_dataset(Path::new("/tmp/Unsafe Name.ndjson"));

    assert_eq!(
        path,
        PathBuf::from("artifacts/motion-calibrations/unsafe-name-calibration-v1.json")
    );
}

#[test]
fn human_report_mentions_residuals_and_quality() {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let report = calibrate_dataset_str(&dataset, "stationary.ndjson").expect("calibration");
    let rendered = format_human_report(&report, Path::new("stationary.ndjson"));

    assert!(rendered.contains("quality: pass"));
    assert!(rendered.contains("gravityResidualAngle"));
    assert!(rendered.contains("yawCalibrated: false"));
}

fn assert_violation(err: &CalibrationError, expected_fragment: &str) {
    match err {
        CalibrationError::QualityGateFailed(report) => {
            let rendered = serde_json::to_string(report).expect("render quality report");
            assert!(
                rendered.contains(expected_fragment),
                "missing fragment {expected_fragment} in {rendered}"
            );
        }
        other => panic!("expected quality gate error, got {other}"),
    }
}

fn valid_profile_value() -> serde_json::Value {
    let dataset = build_stationary_dataset(StationarySpec::default());
    let profile =
        calibrate_dataset_str_at(&dataset, "profile.ndjson", fixed_profile_creation_time())
            .expect("calibration")
            .profile;
    serde_json::to_value(profile).expect("profile value")
}

fn assert_tampered_profile_rejected_everywhere(name: &str, value: serde_json::Value) {
    let json = serde_json::to_string(&value).expect("json");
    assert!(
        load_calibration_profile_str(&json).is_err(),
        "load should reject tampered profile: {name}"
    );

    let profile: anchor_desktop_lib::calibration::CalibrationProfileV1 =
        serde_json::from_value(value).expect("tampered profile still has the profile shape");
    let output = temp_path(&format!("tampered-{name}")).join("profile.json");
    fs::create_dir_all(output.parent().expect("parent")).expect("dir");
    assert!(
        write_calibration_profile_file(&output, &profile).is_err(),
        "write should reject tampered profile: {name}"
    );
    assert!(
        apply_calibration_to_linear(&profile, &vec3(0.0, 0.0, 0.0)).is_err(),
        "linear apply should reject tampered profile: {name}"
    );
    assert!(
        apply_calibration_to_angular(&profile, &vec3(0.0, 0.0, 0.0)).is_err(),
        "angular apply should reject tampered profile: {name}"
    );
    assert!(
        apply_calibration_to_gravity(&profile, &vec3(0.0, 0.0, -9.81)).is_err(),
        "gravity apply should reject tampered profile: {name}"
    );
}

fn fixed_profile_creation_time() -> SystemTime {
    let parsed = time::OffsetDateTime::parse(
        "2026-09-03T12:00:01Z",
        &time::format_description::well_known::Rfc3339,
    )
    .expect("fixed RFC3339 time");
    parsed.into()
}

#[derive(Clone)]
struct StationarySpec {
    gravity_device: Vector3,
    linear_bias: Vector3,
    angular_bias: Vector3,
    sample_count: usize,
}

impl Default for StationarySpec {
    fn default() -> Self {
        Self {
            gravity_device: vec3(0.0, 0.0, -9.81),
            linear_bias: vec3(0.0, 0.0, 0.0),
            angular_bias: vec3(0.0, 0.0, 0.0),
            sample_count: 240,
        }
    }
}

fn build_stationary_dataset(spec: StationarySpec) -> String {
    build_dataset(BuildDatasetSpec {
        sample_count: spec.sample_count,
        gravity_device: spec.gravity_device,
        linear_bias: spec.linear_bias,
        angular_bias: spec.angular_bias,
        ..BuildDatasetSpec::stationary_defaults()
    })
}

struct BuildDatasetSpec {
    scenario: RecordingScenario,
    sample_count: usize,
    completed: bool,
    session_interval_us: u64,
    received_interval_us: u64,
    gravity_device: Vector3,
    gravity_noise_amplitude: f64,
    linear_bias: Vector3,
    angular_bias: Vector3,
    split_session_at: Option<usize>,
    sequence_jump_at: Option<(usize, u32)>,
    recorder_dropped_samples: u64,
}

impl BuildDatasetSpec {
    fn stationary_defaults() -> Self {
        Self {
            scenario: RecordingScenario::Stationary,
            sample_count: 240,
            completed: true,
            session_interval_us: 16_667,
            received_interval_us: 16_667,
            gravity_device: vec3(0.0, 0.0, -9.81),
            gravity_noise_amplitude: 0.0,
            linear_bias: vec3(0.0, 0.0, 0.0),
            angular_bias: vec3(0.0, 0.0, 0.0),
            split_session_at: None,
            sequence_jump_at: None,
            recorder_dropped_samples: 0,
        }
    }
}

fn build_dataset(spec: BuildDatasetSpec) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "{{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"{}\",\"startedAtUtc\":\"2026-09-03T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"{}\"}}",
        spec.scenario.as_str(),
        DEFAULT_MOUNTING_CONVENTION,
    ));

    let mut session_elapsed_us = 0_u64;
    let mut received_elapsed_us = 0_u64;
    let mut sequence = 0_u32;
    for index in 0..spec.sample_count {
        let session_id = if spec.split_session_at.is_some_and(|split| index >= split) {
            "session-b"
        } else {
            "session-a"
        };

        if index > 0 {
            session_elapsed_us = session_elapsed_us.saturating_add(spec.session_interval_us);
            received_elapsed_us = received_elapsed_us.saturating_add(spec.received_interval_us);
        }

        if let Some((jump_at, delta)) = spec.sequence_jump_at {
            if index == jump_at {
                sequence = sequence.saturating_add(delta);
            }
        }

        let gravity_noise = if spec.gravity_noise_amplitude == 0.0 {
            0.0
        } else if index % 2 == 0 {
            spec.gravity_noise_amplitude
        } else {
            -spec.gravity_noise_amplitude
        };

        let sample = MotionSampleV1 {
            protocol_version: ProtocolVersion::V1,
            kind: PacketKind::MotionSample,
            session_id: session_id.to_owned(),
            sequence,
            session_elapsed_us,
            linear_acceleration_mps2: spec.linear_bias.clone(),
            gravity_mps2: vec3(
                spec.gravity_device.x,
                spec.gravity_device.y,
                spec.gravity_device.z + gravity_noise,
            ),
            angular_velocity_rad_s: spec.angular_bias.clone(),
        };

        lines.push(format!(
            "{{\"recordType\":\"sample\",\"receivedElapsedUs\":{},\"sample\":{}}}",
            received_elapsed_us,
            serde_json::to_string(&sample).expect("serialize sample"),
        ));

        sequence = sequence.saturating_add(1);
    }

    lines.push(format!(
        "{{\"recordType\":\"summary\",\"completed\":{},\"durationUs\":{},\"receivedAcceptedSamples\":{},\"writtenSamples\":{},\"recorderDroppedSamples\":{}}}",
        if spec.completed { "true" } else { "false" },
        session_elapsed_us,
        spec.sample_count as u64 + spec.recorder_dropped_samples,
        spec.sample_count,
        spec.recorder_dropped_samples,
    ));

    lines.join("\n") + "\n"
}

fn write_dataset_file(name: &str, contents: &str) -> PathBuf {
    let path = temp_path(name).join("dataset.ndjson");
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
    fs::write(&path, contents).expect("write dataset");
    path
}

fn temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("anchor-b2-{name}-{}-{nanos}", std::process::id()))
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_anchor-motion-dataset"))
        .args(args)
        .output()
        .expect("run cli")
}

fn list_matching(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().contains(suffix))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn euler_rotation_deg(roll_deg: f64, pitch_deg: f64, yaw_deg: f64) -> [f64; 4] {
    let (sr, cr) = (deg_to_rad(roll_deg) * 0.5).sin_cos();
    let (sp, cp) = (deg_to_rad(pitch_deg) * 0.5).sin_cos();
    let (sy, cy) = (deg_to_rad(yaw_deg) * 0.5).sin_cos();

    [
        cr * cp * cy + sr * sp * sy,
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
    ]
}

fn rotate_vector(quaternion: [f64; 4], vector: Vector3) -> Vector3 {
    let [w, x, y, z] = quaternion;
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let wx = w * x;
    let wy = w * y;
    let wz = w * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    vec3(
        (1.0 - 2.0 * (yy + zz)) * vector.x
            + 2.0 * (xy - wz) * vector.y
            + 2.0 * (xz + wy) * vector.z,
        2.0 * (xy + wz) * vector.x
            + (1.0 - 2.0 * (xx + zz)) * vector.y
            + 2.0 * (yz - wx) * vector.z,
        2.0 * (xz - wy) * vector.x
            + 2.0 * (yz + wx) * vector.y
            + (1.0 - 2.0 * (xx + yy)) * vector.z,
    )
}

fn deg_to_rad(value: f64) -> f64 {
    value * PI / 180.0
}

fn vec3(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3 { x, y, z }
}

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected} +/- {tolerance}, got {actual}"
    );
}
